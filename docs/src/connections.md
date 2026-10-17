# Connections

## Adding a connection

Choose **New Connection** from the File menu, the menu at the top of the
sidebar or the command palette. Pick the database type, fill in the host,
port, user, password and database, and click **Test Connection** to check
it. **Save** adds it to the sidebar. Click it there to connect.

SQLite and DuckDB connections only need a file, see
[SQLite and DuckDB files](file-databases.md).

Right click a connection for **Edit**, **Duplicate**, **Refresh**,
**Disconnect** and **Remove**.

## Passwords

Passwords are stored in the operating system's keychain, never in Blanco's
own files.

- Check **Ask on connect** to be asked for the password each time instead.
  Blanco keeps it until you disconnect.
- If the server turns a password down, Blanco asks for it again.
- A PostgreSQL or CockroachDB connection without a password reads the
  password file, the same way `psql` does (`PGPASSFILE`, else `~/.pgpass`).

## SSH tunnels

Turn on SSH to reach a database through a bastion host. Blanco signs in with a
password, a key file or keys from your SSH agent, and reads host aliases and
jump hosts from `~/.ssh/config`. The host key is checked against
`known_hosts`. The SSH password and the key passphrase can also be asked for on
connect.

## TLS

PostgreSQL, CockroachDB and MySQL connections take an SSL mode and optional CA
certificate, client certificate and key files. CockroachDB Cloud needs
`verify-full` with the cluster's CA certificate.

## Environments

Each connection can carry an environment tag: LOCAL, DEV, TEST, STAGING or
PROD, or one you add yourself. The tag colours the connection in the sidebar
and its tabs. PROD asks before anything that writes. See
[Environments and safety](safety.md).

## Idle connections

A connection that has been idle for a while is closed and opened again when
you next use it. Set the time in Settings, and override it per connection in
the connection's form.

## Moving connections to another machine

Use **Export** in the sidebar menu to save all connections, or one from its
own menu, to a JSON file. Passwords are not included. **Import** reads such a
file, lists what it will add and skips connections you already have.
