use sqlparser::ast::Statement;
use sqlparser::dialect::{Dialect, MySqlDialect};
use sqlparser::parser::Parser as SqlParser;

use crate::sql::error::{Error, Result};

pub struct Parser;

impl Parser {
    pub fn parse_sql(sql: &str) -> Result<Vec<Statement>> {
        let dialect = MySqlDialect {};
        let statements = SqlParser::parse_sql(&dialect, sql)?;
        Ok(statements)
    }

    #[allow(dead_code)]
    pub fn parse_one(sql: &str) -> Result<Statement> {
        let mut statements = Self::parse_sql(sql)?;
        if statements.len() != 1 {
            return Err(Error::Parse(
                sqlparser::parser::ParserError::ParserError(format!(
                    "expected 1 statement, got {}",
                    statements.len()
                )),
            ));
        }
        Ok(statements.remove(0))
    }

    #[allow(dead_code)]
    pub fn with_dialect(sql: &str, dialect: &dyn Dialect) -> Result<Vec<Statement>> {
        let statements = SqlParser::parse_sql(dialect, sql)?;
        Ok(statements)
    }
}
