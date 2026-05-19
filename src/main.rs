mod sql;

fn main() -> sql::Result<()> {
    let sql = "SELECT id, name, email FROM users WHERE age > 18 ORDER BY name LIMIT 10";

    let tokens = sql::Tokenizer::new(sql).tokenize()?;
    println!("=== Tokens ===");
    for token in &tokens {
        println!("  {token:?}");
    }

    let statements = sql::Parser::parse_sql(sql)?;
    println!("\n=== AST ===");
    for stmt in &statements {
        println!("  {stmt:#?}");
    }

    let single = sql::Parser::parse_one("INSERT INTO users (id, name) VALUES (1, 'Alice')")?;
    println!("\n=== Single statement ===");
    println!("  {single:#?}");

    Ok(())
}
