use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mysql_async::prelude::*;
use mysql_async::OptsBuilder;

const TEST_PORT: u16 = 3307;

static SERVER_READY: AtomicBool = AtomicBool::new(false);

fn start_server() {
    let catalog = std::sync::Arc::new(std::sync::Mutex::new(srrdb::engine::catalog::Catalog::new()));
    let storage = std::sync::Arc::new(std::sync::Mutex::new(srrdb::engine::storage::Storage::new()));
    let executor = std::sync::Arc::new(srrdb::engine::executor::Executor::new(catalog, storage));

    let ready = Arc::new(AtomicBool::new(false));
    let r = ready.clone();

    let addr = format!("127.0.0.1:{TEST_PORT}");
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
        r.store(true, Ordering::Release);
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let ex = executor.clone();
            tokio::spawn(async move {
                let _ = srrdb::server::connection::handle_connection(stream, ex, None).await;
            });
        }
    });

    while !ready.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(10));
    }
}

async fn connect() -> (mysql_async::Pool, mysql_async::Conn) {
    let opts = OptsBuilder::default()
        .ip_or_hostname("127.0.0.1")
        .tcp_port(TEST_PORT)
        .user(Some("root"))
        .db_name(Some("srrdb"));

    let pool = mysql_async::Pool::new(opts);
    let conn = pool.get_conn().await.expect("connect failed");
    (pool, conn)
}

async fn setup_items(conn: &mut mysql_async::Conn) {
    conn.query_drop("DROP TABLE IF EXISTS items").await.unwrap();
    conn.query_drop(
        "CREATE TABLE items (id INT, name TEXT, price DOUBLE, stock INT)"
    ).await.unwrap();

    conn.query_drop("INSERT INTO items VALUES (1, 'Apple', 1.5, 100)").await.unwrap();
    conn.query_drop("INSERT INTO items VALUES (2, 'Banana', 0.8, 50)").await.unwrap();
    conn.query_drop("INSERT INTO items VALUES (3, 'Cherry', 2.5, 75)").await.unwrap();
    conn.query_drop("INSERT INTO items VALUES (4, 'Date', 3.0, 20)").await.unwrap();
    conn.query_drop("INSERT INTO items VALUES (5, 'Elderberry', 4.5, 10)").await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_all_features() {
    if !SERVER_READY.load(Ordering::Acquire) {
        start_server();
        SERVER_READY.store(true, Ordering::Release);
    }

    // ===== Basic CRUD =====
    let (_pool, mut conn) = connect().await;

    conn.query_drop("CREATE TABLE test (id INT, name TEXT)")
        .await
        .expect("CREATE TABLE failed");

    conn.query_drop("INSERT INTO test VALUES (1, 'Alice')")
        .await
        .expect("INSERT failed");

    conn.query_drop("INSERT INTO test VALUES (2, 'Bob')")
        .await
        .expect("INSERT failed");

    let rows: Vec<(i32, String)> = conn
        .query("SELECT id, name FROM test ORDER BY id")
        .await
        .expect("SELECT failed");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], (1, "Alice".to_string()));
    assert_eq!(rows[1], (2, "Bob".to_string()));

    // ===== DELETE =====
    setup_items(&mut conn).await;

    conn.query_drop("DELETE FROM items WHERE id = 1").await.unwrap();
    let rows: Vec<(i32, String)> = conn
        .query("SELECT id, name FROM items ORDER BY id")
        .await
        .unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].0, 2);

    conn.query_drop("DELETE FROM items WHERE price > 3.0").await.unwrap();
    let rows: Vec<(i32, String)> = conn
        .query("SELECT id, name FROM items ORDER BY id")
        .await
        .unwrap();
    assert_eq!(rows.len(), 3);

    conn.query_drop("DELETE FROM items").await.unwrap();
    let rows: Vec<(i32, String)> = conn
        .query("SELECT id, name FROM items ORDER BY id")
        .await
        .unwrap();
    assert_eq!(rows.len(), 0);

    // ===== UPDATE =====
    setup_items(&mut conn).await;

    conn.query_drop("UPDATE items SET price = 1.0 WHERE name = 'Banana'").await.unwrap();
    let price: f64 = conn
        .query_first("SELECT price FROM items WHERE name = 'Banana'")
        .await
        .unwrap()
        .unwrap();
    assert!((price - 1.0).abs() < 0.001);

    conn.query_drop("UPDATE items SET stock = 0 WHERE stock < 30").await.unwrap();
    let rows: Vec<(i32, i32)> = conn
        .query("SELECT id, stock FROM items WHERE stock = 0 ORDER BY id")
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, 4);
    assert_eq!(rows[1].0, 5);

    // ===== ORDER BY =====
    setup_items(&mut conn).await;

    let rows: Vec<(String, f64)> = conn
        .query("SELECT name, price FROM items ORDER BY price")
        .await
        .unwrap();
    assert_eq!(rows[0].0, "Banana");
    assert_eq!(rows[4].0, "Elderberry");

    let rows: Vec<String> = conn
        .query("SELECT name FROM items ORDER BY name DESC")
        .await
        .unwrap();
    assert_eq!(rows[0], "Elderberry");
    assert_eq!(rows[4], "Apple");

    // ===== LIMIT / OFFSET =====
    setup_items(&mut conn).await;

    let rows: Vec<(i32, String)> = conn
        .query("SELECT id, name FROM items ORDER BY id LIMIT 2")
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, 1);
    assert_eq!(rows[1].0, 2);

    let rows: Vec<(i32, String)> = conn
        .query("SELECT id, name FROM items ORDER BY id LIMIT 2 OFFSET 2")
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, 3);
    assert_eq!(rows[1].0, 4);

    // ===== LIKE =====
    setup_items(&mut conn).await;

    let rows: Vec<String> = conn
        .query("SELECT name FROM items WHERE name LIKE 'A%' ORDER BY name")
        .await
        .unwrap();
    assert_eq!(rows, vec!["Apple"]);

    let rows: Vec<String> = conn
        .query("SELECT name FROM items WHERE name LIKE '%erry' ORDER BY name")
        .await
        .unwrap();
    assert_eq!(rows, vec!["Cherry", "Elderberry"]);

    let rows: Vec<String> = conn
        .query("SELECT name FROM items WHERE name LIKE '%a%' ORDER BY name")
        .await
        .unwrap();
    // Apple, Banana, Date all contain 'a' (case-insensitive)
    assert_eq!(rows, vec!["Apple", "Banana", "Date"]);

    // ===== BETWEEN =====
    setup_items(&mut conn).await;

    let rows: Vec<String> = conn
        .query("SELECT name FROM items WHERE price BETWEEN 2.0 AND 4.0 ORDER BY name")
        .await
        .unwrap();
    assert_eq!(rows, vec!["Cherry", "Date"]);

    // ===== IN =====
    setup_items(&mut conn).await;

    let rows: Vec<String> = conn
        .query("SELECT name FROM items WHERE name IN ('Apple', 'Cherry', 'Fig') ORDER BY name")
        .await
        .unwrap();
    assert_eq!(rows, vec!["Apple", "Cherry"]);

    // ===== IS NULL / IS NOT NULL =====
    setup_items(&mut conn).await;
    conn.query_drop("INSERT INTO items VALUES (6, 'Fig', NULL, NULL)").await.unwrap();

    let rows: Vec<String> = conn
        .query("SELECT name FROM items WHERE price IS NULL ORDER BY name")
        .await
        .unwrap();
    assert_eq!(rows, vec!["Fig"]);

    let rows: Vec<String> = conn
        .query("SELECT name FROM items WHERE price IS NOT NULL ORDER BY name")
        .await
        .unwrap();
    assert_eq!(rows.len(), 5);

    // ===== INSERT with column list =====
    conn.query_drop("DROP TABLE IF EXISTS coltest").await.unwrap();
    conn.query_drop("CREATE TABLE coltest (id INT, name TEXT, price DOUBLE)")
        .await
        .unwrap();

    conn.query_drop("INSERT INTO coltest (name, id) VALUES ('Bob', 2)")
        .await
        .unwrap();
    conn.query_drop("INSERT INTO coltest (name, id, price) VALUES ('Alice', 1, 9.99)")
        .await
        .unwrap();

    let rows: Vec<(i32, Option<String>, Option<f64>)> = conn
        .query("SELECT id, name, price FROM coltest ORDER BY id")
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], (1, Some("Alice".to_string()), Some(9.99)));
    assert_eq!(rows[1], (2, Some("Bob".to_string()), None));

    // ===== CREATE TABLE IF NOT EXISTS =====
    conn.query_drop("CREATE TABLE IF NOT EXISTS coltest (id INT)")
        .await
        .unwrap();
    // Table still has 3 columns, not 1 — verify it wasn't recreated
    let rows: Vec<(i32, Option<String>, Option<f64>)> = conn
        .query("SELECT id, name, price FROM coltest ORDER BY id")
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);

    // ===== Database management =====
    conn.query_drop("CREATE DATABASE IF NOT EXISTS blogtest")
        .await
        .unwrap();
    conn.query_drop("USE blogtest").await.unwrap();
    conn.query_drop("CREATE TABLE t (a INT)").await.unwrap();
    conn.query_drop("INSERT INTO t VALUES (1)").await.unwrap();
    let val: i32 = conn.query_first("SELECT a FROM t").await.unwrap().unwrap();
    assert_eq!(val, 1);

    // if_not_exists should not error
    conn.query_drop("CREATE DATABASE IF NOT EXISTS blogtest")
        .await
        .unwrap();
    conn.query_drop("CREATE TABLE IF NOT EXISTS t (a INT)")
        .await
        .unwrap();

    // DROP DATABASE IF EXISTS
    conn.query_drop("DROP DATABASE IF EXISTS nonexistent")
        .await
        .unwrap();
    conn.query_drop("DROP DATABASE blogtest").await.unwrap();

    // Switch back to srrdb for cleanup
    conn.query_drop("USE srrdb").await.unwrap();

    drop(conn);
}
