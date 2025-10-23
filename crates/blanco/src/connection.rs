use gpui::SharedString;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Connection {
    pub id: usize,
    pub name: SharedString,
    pub host: SharedString,
    pub port: u16,
    pub database: SharedString,
    pub schemas: Vec<Schema>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Schema {
    pub name: SharedString,
    pub tables: Vec<Table>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Table {
    pub name: SharedString,
}

impl Connection {
    pub fn new_mock() -> Vec<Self> {
        vec![
            Connection {
                id: 0,
                name: "Local PostgreSQL".into(),
                host: "localhost".into(),
                port: 5432,
                database: "mydb".into(),
                schemas: vec![
                    Schema {
                        name: "public".into(),
                        tables: vec![
                            Table {
                                name: "users".into(),
                            },
                            Table {
                                name: "posts".into(),
                            },
                            Table {
                                name: "comments".into(),
                            },
                        ],
                    },
                    Schema {
                        name: "auth".into(),
                        tables: vec![
                            Table {
                                name: "sessions".into(),
                            },
                            Table {
                                name: "tokens".into(),
                            },
                        ],
                    },
                ],
            },
            Connection {
                id: 1,
                name: "Production DB".into(),
                host: "prod.example.com".into(),
                port: 5432,
                database: "production".into(),
                schemas: vec![Schema {
                    name: "public".into(),
                    tables: vec![
                        Table {
                            name: "customers".into(),
                        },
                        Table {
                            name: "orders".into(),
                        },
                        Table {
                            name: "products".into(),
                        },
                    ],
                }],
            },
        ]
    }
}
