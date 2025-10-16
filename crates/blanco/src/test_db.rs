use crate::database::DatabaseManager;
use sqlx::Row;
use std::path::PathBuf;

/// Get the path to the test database
pub fn test_db_path() -> PathBuf {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("blanco");

    std::fs::create_dir_all(&config_dir).ok();
    config_dir.join("test.db")
}

/// Initialize the test database with sample data
pub async fn init_test_database(
    db: &mut DatabaseManager,
) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = test_db_path();
    let db_path_str = format!("sqlite://{}", db_path.display());

    println!("Initializing test database at: {}", db_path.display());

    // Connect to the database
    db.connect_async(&db_path_str).await?;

    // Use async operations directly (caller should handle runtime)
    {
        // Create sample tables using the async method
        let pool = db.pool.as_ref().ok_or("Not connected to database")?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                email TEXT NOT NULL UNIQUE,
                age INTEGER,
                city TEXT,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            )",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS orders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                product TEXT NOT NULL,
                quantity INTEGER NOT NULL,
                price REAL NOT NULL,
                order_date TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (user_id) REFERENCES users(id)
            )",
        )
        .execute(pool)
        .await?;

        // Check if we need to insert sample data
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM users")
            .fetch_one(pool)
            .await?;
        let count: i64 = count_result.try_get("count")?;
        let has_data = count > 0;

        if !has_data {
            println!("Inserting sample data...");

            // Insert sample users (some with NULL values for testing)
            sqlx::query(
                "INSERT INTO users (name, email, age, city) VALUES
                ('Alice Johnson', 'alice@example.com', 28, 'San Francisco'),
                ('Bob Smith', 'bob@example.com', 34, 'New York'),
                ('Charlie Brown', 'charlie@example.com', NULL, 'Austin'),
                ('Diana Prince', 'diana@example.com', 31, NULL),
                ('Eve Williams', 'eve@example.com', NULL, NULL)",
            )
            .execute(pool)
            .await?;

            // Insert sample orders
            sqlx::query(
                "INSERT INTO orders (user_id, product, quantity, price) VALUES
                (1, 'Laptop', 1, 1299.99),
                (1, 'Mouse', 2, 29.99),
                (2, 'Keyboard', 1, 89.99),
                (3, 'Monitor', 2, 349.99),
                (3, 'Webcam', 1, 79.99),
                (4, 'Headphones', 1, 199.99),
                (5, 'Desk Chair', 1, 299.99)",
            )
            .execute(pool)
            .await?;

            println!("Sample data inserted successfully!");
        }
    }
    Ok(())
}
