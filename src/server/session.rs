#[derive(Debug, Clone)]
pub struct User {
    pub name: String,
    pub password_hash: Option<[u8; 20]>,
}

impl User {
    pub fn new(name: &str) -> Self {
        User { name: name.to_string(), password_hash: None }
    }

    pub fn with_password(name: &str, password: &str) -> Self {
        let hash = crate::protocol::handshake::hash_native_password(password);
        User { name: name.to_string(), password_hash: Some(hash) }
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub user: User,
    pub database: String,
}

impl Session {
    pub fn new(user: &str) -> Self {
        Session {
            user: User::new(user),
            database: "srrdb".to_string(),
        }
    }

    pub fn new_with_user(user: User) -> Self {
        Session {
            user,
            database: "srrdb".to_string(),
        }
    }
}
