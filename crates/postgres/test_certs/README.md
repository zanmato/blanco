# SSL Test Certificates

This directory contains test certificates for SSL/TLS connection testing with PostgreSQL.

## Files

- `ca.crt` - CA certificate used to verify the server certificate
- `ca.key` - CA private key (not needed for tests, kept for regeneration)
- `server.crt` - Server certificate signed by the CA
- `server.key` - Server private key

## Regenerating Certificates

If you need to regenerate these certificates:

```bash
# Generate CA private key
openssl genrsa -out ca.key 4096

# Generate CA certificate
openssl req -new -x509 -days 3650 -key ca.key -out ca.crt -subj "/CN=Blanco Test CA"

# Generate server private key
openssl genrsa -out server.key 2048

# Generate server CSR
openssl req -new -key server.key -out server.csr -subj "/CN=localhost"

# Sign server certificate with CA
openssl x509 -req -days 3650 -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out server.crt

# Set proper permissions
chmod 644 ca.crt server.crt
chmod 600 ca.key server.key
rm server.csr
```

## Usage

These certificates are mounted into the `postgrestestdb_ssl` Docker container and used for SSL connection testing. The tests use `ca.crt` to verify the server certificate with `sslmode=verify-ca`.
