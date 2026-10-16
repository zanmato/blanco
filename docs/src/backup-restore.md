# Backup and restore

Back up and restore PostgreSQL, MySQL and MariaDB databases with the tools you
already have: `pg_dump`, `pg_restore` and `psql`, or `mysqldump` and `mysql`.
Blanco finds them on your `PATH`. Set their paths in Settings if they live
elsewhere.

## Back up

Right click a database and choose **Back Up**, or find it in the command
palette.

- Pick the tables, and for each whether to include data or structure only.
- For PostgreSQL, pick the plain, custom or directory format.
- Compress with gzip, from fast to smallest.
- Name the file with a pattern, `{database}_{date}` by default, and pick the
  folder. Both are remembered.

The tool's output shows while it runs, and **Stop** cancels it. You get a
notification when it finishes.

## Restore

Right click a database and choose **Restore**. Blanco looks at the file,
whether a plain or gzipped SQL dump, a custom archive or a directory, and runs
the matching tool.
