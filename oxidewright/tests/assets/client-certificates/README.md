Test-only TLS material for `tests/integration/client_certificates.rs`:
a throwaway CA (its key was discarded), a `localhost` server certificate and
an "Alice" client certificate it signed, plus a self-signed "Bob" certificate.
Never use these keys for anything else.
