use crate::database::DatabaseManager;
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
pub async fn init_test_database(db: &mut DatabaseManager) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = test_db_path();
    let db_path_str = format!("sqlite://{}", db_path.display());

    println!("Initializing test database at: {}", db_path.display());

    // Connect to the database
    db.connect(&db_path_str).await?;

    // Create sample tables
    db.execute_query(
        "CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            email TEXT NOT NULL UNIQUE,
            age INTEGER,
            city TEXT,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )"
    ).await?;

    db.execute_query(
        "CREATE TABLE IF NOT EXISTS orders (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER NOT NULL,
            product TEXT NOT NULL,
            quantity INTEGER NOT NULL,
            price REAL NOT NULL,
            order_date TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (user_id) REFERENCES users(id)
        )"
    ).await?;

    // Check if we need to insert sample data
    let users_result = db.execute_query("SELECT COUNT(*) as count FROM users").await?;
    let has_data = users_result.rows.get(0)
        .and_then(|row| row.get(0))
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0) > 0;

    if !has_data {
        println!("Inserting sample data...");

        // Insert sample users
        db.execute_query(
            "INSERT INTO users (name, email, age, city) VALUES
            ('Alice Johnson', 'alice@example.com', 28, 'San Francisco'),
            ('Bob Smith', 'bob@example.com', 34, 'New York'),
            ('Charlie Brown', 'charlie@example.com', 25, 'Austin'),
            ('Diana Prince', 'diana@example.com', 31, 'Seattle'),
            ('Eve Williams', 'eve@example.com', 29, 'Portland')"
        ).await?;

        // Insert sample orders
        db.execute_query(
            "INSERT INTO orders (user_id, product, quantity, price) VALUES
            (1, 'Laptop', 1, 1299.99),
            (1, 'Mouse', 2, 29.99),
            (2, 'Keyboard', 1, 89.99),
            (3, 'Monitor', 2, 349.99),
            (3, 'Webcam', 1, 79.99),
            (4, 'Headphones', 1, 199.99),
            (5, 'Desk Chair', 1, 299.99)"
        ).await?;

        println!("Sample data inserted successfully!");
    }

    Ok(())
}
