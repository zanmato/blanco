-- The demo database the website screenshots are taken against: an invented
-- online record shop. Every value is derived from row numbers, so each run
-- produces the same data and the same screenshots.
--
-- Usage: sqlite3 demo.db < scripts/screenshots/demo.sql

PRAGMA foreign_keys = ON;

CREATE TABLE genres (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE artists (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    country TEXT NOT NULL,
    formed_year INTEGER
);

CREATE TABLE albums (
    id INTEGER PRIMARY KEY,
    artist_id INTEGER NOT NULL REFERENCES artists (id),
    genre_id INTEGER NOT NULL REFERENCES genres (id),
    title TEXT NOT NULL,
    release_year INTEGER NOT NULL,
    format TEXT NOT NULL CHECK (format IN ('LP', '2LP', '7"', '12"')),
    price NUMERIC(6, 2) NOT NULL,
    stock INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE customers (
    id INTEGER PRIMARY KEY,
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    city TEXT NOT NULL,
    joined_at TEXT NOT NULL
);

CREATE TABLE orders (
    id INTEGER PRIMARY KEY,
    customer_id INTEGER NOT NULL REFERENCES customers (id),
    ordered_at TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'shipped', 'delivered', 'returned'))
);

CREATE TABLE order_items (
    order_id INTEGER NOT NULL REFERENCES orders (id),
    album_id INTEGER NOT NULL REFERENCES albums (id),
    quantity INTEGER NOT NULL,
    unit_price NUMERIC(6, 2) NOT NULL,
    PRIMARY KEY (order_id, album_id)
);

CREATE TABLE reviews (
    id INTEGER PRIMARY KEY,
    album_id INTEGER NOT NULL REFERENCES albums (id),
    customer_id INTEGER NOT NULL REFERENCES customers (id),
    rating INTEGER NOT NULL CHECK (rating BETWEEN 1 AND 5),
    body TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX orders_customer_id ON orders (customer_id);
CREATE INDEX order_items_album_id ON order_items (album_id);
CREATE INDEX reviews_album_id ON reviews (album_id);

CREATE VIEW album_sales AS
SELECT
    al.id AS album_id,
    al.title,
    ar.name AS artist,
    SUM(oi.quantity) AS copies,
    SUM(oi.quantity * oi.unit_price) AS revenue
FROM albums al
JOIN artists ar ON ar.id = al.artist_id
JOIN order_items oi ON oi.album_id = al.id
GROUP BY al.id;

INSERT INTO genres (id, name) VALUES
    (1, 'Jazz'), (2, 'Soul'), (3, 'Funk'), (4, 'Ambient'), (5, 'Post-punk'),
    (6, 'Krautrock'), (7, 'Folk'), (8, 'Dub'), (9, 'Afrobeat'), (10, 'Shoegaze'),
    (11, 'Bossa Nova'), (12, 'Electronic');

-- Word lists the generators below pick from by row number.
CREATE TEMP TABLE words (list TEXT, ix INTEGER, word TEXT);
INSERT INTO words (list, ix, word)
SELECT 'adjective', key, value FROM json_each('["Velvet","Silent","Golden","Electric","Midnight","Paper","Crystal","Hollow","Northern","Lunar","Copper","Wild","Distant","Neon","Quiet","Burning","Glass","Endless","Faded","Static"]')
UNION ALL
SELECT 'noun', key, value FROM json_each('["Harbour","Gardens","Signal","Orchestra","Tides","Machines","Rivers","Choir","Satellites","Lanterns","Echoes","Pilots","Weather","Collective","Horizon","Ensemble","Arcade","Foxes","Ballroom","Parade"]')
UNION ALL
SELECT 'title', key, value FROM json_each('["Low Light","Slow Motion","After Hours","Open Water","Second Nature","Long Distance","Night Drive","Blue Room","Spring Tide","Small Hours","Heat Haze","Far Shore","Stereo Field","Late Bloom","Soft Focus","High Plains","Dream State","Paper Moon","Sea Level","Last Train"]')
UNION ALL
SELECT 'first', key, value FROM json_each('["Alma","Bruno","Clara","Dante","Elsa","Felix","Greta","Hugo","Iris","Jonas","Karin","Leo","Maja","Nils","Olivia","Pablo","Rosa","Sami","Tilda","Viktor","Wilma","Yusuf","Zoe","Axel","Bea"]')
UNION ALL
SELECT 'last', key, value FROM json_each('["Lindqvist","Moreau","Okafor","Bianchi","Nakamura","Svensson","Kowalski","Fischer","Haddad","Costa","Jansen","Novak","Berg","Duarte","Eriksson","Larsen","Rossi","Weber","Holm","Ivanova"]')
UNION ALL
SELECT 'city', key, value FROM json_each('["Stockholm","Gothenburg","Malmö","Copenhagen","Oslo","Helsinki","Berlin","Hamburg","Amsterdam","London","Manchester","Paris","Lyon","Barcelona","Lisbon","Vienna"]')
UNION ALL
SELECT 'country', key, value FROM json_each('["SE","GB","US","DE","FR","JP","NG","BR","NO","IS"]')
UNION ALL
SELECT 'review', key, value FROM json_each('["Warm pressing, quiet surfaces.","Side B is the reason to own this.","Arrived well packed and dead flat.","A little sibilance on the inner grooves.","Better than the original release.","Gatefold artwork alone is worth it.","Slow burner, now on constant rotation.","Bass is huge on a decent system.","Some surface noise on the first track.","Exactly what I hoped for."]');

CREATE TEMP VIEW counts AS
SELECT list, COUNT(*) AS n FROM words GROUP BY list;

WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 240)
INSERT INTO artists (id, name, country, formed_year)
SELECT
    n,
    'The ' || (SELECT word FROM words WHERE list = 'adjective' AND ix = (n * 7) % 20)
        || ' ' || (SELECT word FROM words WHERE list = 'noun' AND ix = (n * 13) % 20),
    (SELECT word FROM words WHERE list = 'country' AND ix = (n * 3) % 10),
    1958 + (n * 17) % 64
FROM seq;

WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 1800)
INSERT INTO albums (id, artist_id, genre_id, title, release_year, format, price, stock)
SELECT
    n,
    1 + (n * 37) % 240,
    -- Weighted, so genres sell in believable proportions.
    CASE
        WHEN (n * 37) % 100 < 18 THEN 1
        WHEN (n * 37) % 100 < 30 THEN 2
        WHEN (n * 37) % 100 < 41 THEN 12
        WHEN (n * 37) % 100 < 50 THEN 4
        WHEN (n * 37) % 100 < 58 THEN 5
        WHEN (n * 37) % 100 < 65 THEN 6
        WHEN (n * 37) % 100 < 72 THEN 7
        WHEN (n * 37) % 100 < 79 THEN 3
        WHEN (n * 37) % 100 < 85 THEN 9
        WHEN (n * 37) % 100 < 90 THEN 8
        WHEN (n * 37) % 100 < 96 THEN 10
        ELSE 11
    END,
    (SELECT word FROM words WHERE list = 'title' AND ix = (n * 11) % 20)
        || CASE WHEN n % 3 = 0 THEN ' Vol. ' || (1 + n % 4) ELSE '' END,
    1962 + (n * 29) % 63,
    CASE n % 9 WHEN 0 THEN '2LP' WHEN 4 THEN '7"' WHEN 7 THEN '12"' ELSE 'LP' END,
    ROUND(19.99 + 2 * ((n * 7) % 12) + CASE WHEN n % 9 = 0 THEN 10 ELSE 0 END, 2),
    (n * 19) % 41
FROM seq;

WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 6000)
INSERT INTO customers (id, first_name, last_name, email, city, joined_at)
SELECT
    n,
    f.word,
    l.word,
    lower(f.word) || '.' || lower(l.word) || n || '@example.com',
    (SELECT word FROM words WHERE list = 'city' AND ix = (n * 7) % 16),
    date('2019-01-01', '+' || ((n * 97) % 2400) || ' days')
FROM seq
JOIN words f ON f.list = 'first' AND f.ix = (n * 7) % 25
JOIN words l ON l.list = 'last' AND l.ix = (n * 11) % 20;

WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 48000)
INSERT INTO orders (id, customer_id, ordered_at, status)
SELECT
    n,
    1 + (n * 131) % 6000,
    datetime('2021-01-01', '+' || ((n * 1789) % 2100) || ' days', '+' || ((n * 7919) % 86400) || ' seconds'),
    CASE WHEN n > 47700 THEN 'pending' WHEN n % 41 = 0 THEN 'returned' WHEN n > 47000 THEN 'shipped' ELSE 'delivered' END
FROM seq;

-- One to four lines per order. Squaring a spread value favours the low album
-- ids, so a few records outsell the rest. A line that repeats an album
-- already in its order is dropped.
WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 48000),
lines(line) AS (VALUES (0), (1), (2), (3)),
picks AS (
    SELECT
        seq.n AS order_id,
        seq.n + lines.line AS q,
        1 + CAST(1800 * ((seq.n * 211 + lines.line * 457) % 1800 / 1800.0)
            * ((seq.n * 211 + lines.line * 457) % 1800 / 1800.0) AS INTEGER) AS album_id
    FROM seq
    JOIN lines ON lines.line <= (seq.n * 3) % 4
)
INSERT OR IGNORE INTO order_items (order_id, album_id, quantity, unit_price)
SELECT picks.order_id, picks.album_id, 1 + picks.q % 7 / 5, al.price
FROM picks
JOIN albums al ON al.id = picks.album_id;

WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM seq WHERE n < 22000)
INSERT INTO reviews (id, album_id, customer_id, rating, body, created_at)
SELECT
    n,
    1 + (n * 307) % 1800,
    1 + (n * 89) % 6000,
    CASE WHEN n % 11 = 0 THEN 2 WHEN n % 5 = 0 THEN 3 WHEN n % 2 = 0 THEN 4 ELSE 5 END,
    (SELECT word FROM words WHERE list = 'review' AND ix = (n * 3) % 10)
        || ' ' || (SELECT word FROM words WHERE list = 'review' AND ix = (n * 7 + 1) % 10),
    date('2021-06-01', '+' || ((n * 61) % 1900) || ' days')
FROM seq;

ANALYZE;
VACUUM;
