use sqlparser::dialect::{Dialect, MySqlDialect};
use sqlparser::tokenizer::{Token, Tokenizer as SqlTokenizer};

use crate::sql::error::Result;

pub struct Tokenizer<'a> {
    sql: &'a str,
    dialect: &'a dyn Dialect,
}

impl<'a> Tokenizer<'a> {
    pub fn new(sql: &'a str) -> Self {
        Self {
            sql,
            dialect: &MySqlDialect {},
        }
    }

    #[allow(dead_code)]
    pub fn with_dialect(sql: &'a str, dialect: &'a dyn Dialect) -> Self {
        Self { sql, dialect }
    }

    pub fn tokenize(&self) -> Result<Vec<Token>> {
        let mut tokenizer = SqlTokenizer::new(self.dialect, self.sql);
        let tokens = tokenizer.tokenize()?;
        Ok(tokens)
    }
}
