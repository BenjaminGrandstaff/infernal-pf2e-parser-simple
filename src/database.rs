//! Goal: own this service's PostgreSQL connection and schema migration,
//! entirely separate from infernal-law's own database -- see this
//! repository's README, "Database boundary". No pgvector, no shared
//! schema, no shared connection pool with the kernel.

use std::env;
use std::fmt::{self, Display, Formatter};

use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use r2d2_postgres::postgres::NoTls;

const DATABASE_URL_ENV: &str = "PF2E_PARSER_DATABASE_URL";

#[derive(Clone)]
pub struct Database {
    pool: Pool<PostgresConnectionManager<NoTls>>,
}

impl Database {
    pub fn connect_from_env() -> Result<Self, DatabaseError> {
        let url = env::var(DATABASE_URL_ENV).map_err(|_| DatabaseError::MissingEnvironment)?;
        if url.trim().is_empty() {
            return Err(DatabaseError::EmptyUrl);
        }
        let config = url
            .parse()
            .map_err(|_| DatabaseError::InvalidPostgresConfig)?;
        let manager = PostgresConnectionManager::new(config, NoTls);
        let pool = Pool::new(manager).map_err(|error| DatabaseError::Pool(error.to_string()))?;
        let database = Self { pool };
        database.check_connection()?;
        database.migrate()?;
        Ok(database)
    }

    pub fn check_connection(&self) -> Result<(), DatabaseError> {
        let mut connection = self
            .pool
            .get()
            .map_err(|error| DatabaseError::Pool(error.to_string()))?;
        connection
            .simple_query("SELECT 1")
            .map_err(|error| DatabaseError::Query(error.to_string()))?;
        Ok(())
    }

    pub fn migrate(&self) -> Result<(), DatabaseError> {
        let mut connection = self
            .pool
            .get()
            .map_err(|error| DatabaseError::Pool(error.to_string()))?;
        connection
            .batch_execute(include_str!("../migrations/0001_init.sql"))
            .map_err(|error| DatabaseError::Query(error.to_string()))?;
        Ok(())
    }

    pub fn connection(
        &self,
    ) -> Result<r2d2::PooledConnection<PostgresConnectionManager<NoTls>>, DatabaseError> {
        self.pool
            .get()
            .map_err(|error| DatabaseError::Pool(error.to_string()))
    }
}

#[derive(Debug)]
pub enum DatabaseError {
    MissingEnvironment,
    EmptyUrl,
    InvalidPostgresConfig,
    Pool(String),
    Query(String),
}

impl Display for DatabaseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnvironment => formatter.write_str("PF2E_PARSER_DATABASE_URL is not set"),
            Self::EmptyUrl => formatter.write_str("PF2E_PARSER_DATABASE_URL is empty"),
            Self::InvalidPostgresConfig => {
                formatter.write_str("PF2E_PARSER_DATABASE_URL is not a valid postgres URL")
            }
            Self::Pool(message) => write!(formatter, "database pool error: {message}"),
            Self::Query(message) => write!(formatter, "database query failed: {message}"),
        }
    }
}
