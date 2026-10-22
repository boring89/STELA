# Test certificates (fixtures)

Only for `make test-tls` and `make test-pkg` (`tools/make_test_certs.py`,
docs/tls.md §5). The keys are public on purpose; never trust `ca.pem`
outside these tests.

- `ca.key`, `ca.pem`: the test root (EC P-256, valid until 2049).
- `good.key`: the servers' key.
- `expired.pem`: for 10.0.2.2, signed by the test root, valid only
  2020-01-01 to 2021-01-01 (the date check). Kept here because making it
  needs `openssl x509 -not_before/-not_after`, which OpenSSL 3.0 lacks.

Made on 2026-10-22 (P14 K0) with OpenSSL 3.5:

    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
        -keyout ca.key -out ca.pem -days 8400 -subj "/CN=STELA test root" \
        -addext basicConstraints=critical,CA:TRUE -addext keyUsage=critical,keyCertSign
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:prime256v1 -out good.key
    openssl req -new -key good.key -subj "/CN=stela test server" -out e.csr
    openssl x509 -req -in e.csr -CA ca.pem -CAkey ca.key -set_serial 2 \
        -extfile e.ext -out expired.pem \
        -not_before 20200101000000Z -not_after 20210101000000Z

(`e.ext`: `subjectAltName=DNS:10.0.2.2,IP:10.0.2.2` and `basicConstraints=CA:FALSE`.)
