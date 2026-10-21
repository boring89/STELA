// Copyright (C) 2026 YuanChi Hsieh
// SPDX-License-Identifier: GPL-3.0-or-later

//! TLS 1.3 client (P12, `docs/tls.md`): `embedded-tls` over a STELA TCP
//! socket, with the certificate chain checked against trusted root
//! certificates (`Initrd:/certs/cacert.der`, Mozilla's list).
//!
//! ```text
//!  request(host, ip, port, bytes, roots)
//!    TCP connect ──► ClientHello (SNI = host, X25519/P-256 share)
//!    ◄── ServerHello, EncryptedExtensions, Certificate, CertificateVerify
//!        Verifier: the root whose subject is the chain's last issuer
//!                  (DER names compared byte for byte; the first such
//!                  root) → embedded-tls's chain check: signatures,
//!                  validity (wall clock), host name (SAN, wildcards)
//!    ──► Finished, the request; ◄── the response until the server closes
//! ```
//!
//! Randomness comes from the kernel (`Random`, ChaCha20 seeded from
//! RDSEED/RDRAND or timing jitter). The DER helpers are pure and tested
//! on the host.

use abi::Error;
use alloc::vec::Vec;

/// Where the system's trusted roots are.
pub const SYSTEM_ROOTS: &str = "Initrd:/certs/cacert.der";

/// A DER element at the start of `data`: (tag, header length, content
/// length). Only definite lengths (DER has no others).
pub fn der_element(data: &[u8]) -> Option<(u8, usize, usize)> {
    let tag = *data.first()?;
    let first = *data.get(1)? as usize;
    if first < 0x80 {
        return (2 + first <= data.len()).then_some((tag, 2, first));
    }
    let n = first & 0x7F;
    if n == 0 || n > 4 {
        return None;
    }
    let mut len = 0usize;
    for i in 0..n {
        len = (len << 8) | *data.get(2 + i)? as usize;
    }
    let header = 2 + n;
    (header.checked_add(len)? <= data.len()).then_some((tag, header, len))
}

/// The certificates in a file of DER certificates put one after another.
pub fn split_certificates(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        match der_element(&data[at..]) {
            Some((0x30, header, len)) => {
                out.push(&data[at..at + header + len]);
                at += header + len;
            }
            _ => break,
        }
    }
    out
}

/// A certificate's issuer and subject names (the whole DER `Name`
/// elements): `Certificate ::= SEQUENCE { tbsCertificate SEQUENCE {
/// [0] version OPTIONAL, serialNumber, signature, issuer, validity,
/// subject, ... } ... }` (RFC 5280 §4.1).
pub fn certificate_names(cert: &[u8]) -> Option<(&[u8], &[u8])> {
    let (tag, header, _) = der_element(cert)?;
    if tag != 0x30 {
        return None;
    }
    let tbs = &cert[header..];
    let (tag, header, len) = der_element(tbs)?;
    if tag != 0x30 {
        return None;
    }
    let mut rest = &tbs[header..header + len];
    let mut fields: Vec<&[u8]> = Vec::new();
    while fields.len() < 6 && !rest.is_empty() {
        let (tag, header, len) = der_element(rest)?;
        if !(fields.is_empty() && tag == 0xA0) {
            fields.push(&rest[..header + len]);
        }
        rest = &rest[header + len..];
    }
    // serial, signature, issuer, validity, subject.
    Some((fields.get(2)?, fields.get(4)?))
}

/// Trusted root certificates (DER, one after another).
pub struct Roots {
    data: Vec<u8>,
}

impl Roots {
    /// Roots from the bytes of a DER bundle.
    pub fn from_der(data: Vec<u8>) -> Roots {
        Roots { data }
    }

    /// Roots read from `path` (`Volume:path`).
    pub fn load(path: &str) -> Result<Roots, Error> {
        Ok(Roots::from_der(crate::fs::read_path(path)?))
    }

    /// The system's roots ([`SYSTEM_ROOTS`]).
    pub fn system() -> Result<Roots, Error> {
        Roots::load(SYSTEM_ROOTS)
    }

    /// The certificates.
    pub fn certificates(&self) -> Vec<&[u8]> {
        split_certificates(&self.data)
    }

    /// The roots that may have signed `cert`: their subject is its
    /// issuer.
    pub fn candidates(&self, cert: &[u8]) -> Vec<&[u8]> {
        let Some((issuer, _)) = certificate_names(cert) else {
            return Vec::new();
        };
        self.certificates()
            .into_iter()
            .filter(|root| certificate_names(root).is_some_and(|(_, s)| s == issuer))
            .collect()
    }
}

#[cfg(target_os = "none")]
mod client {
    use super::*;
    use crate::net::{Ip, Socket};
    use alloc::string::String;
    use alloc::vec;
    use embedded_tls::blocking::TlsConnection;
    use embedded_tls::pki::CertVerifier;
    use embedded_tls::{
        Aes128GcmSha256, Certificate, CertificateEntryRef, CertificateRef, CryptoProvider,
        TlsCipherSuite, TlsClock, TlsConfig, TlsContext, TlsError, TlsVerifier,
    };

    /// Largest server certificate kept for the signature check.
    const CERT_SIZE: usize = 8192;
    /// TLS record buffers (a full record is 16 KiB + overhead).
    const RECORD_BUFFER: usize = 16 * 1024 + 256;

    /// The wall clock for certificate validity (seconds since 1970).
    pub struct WallClock;

    impl TlsClock for WallClock {
        fn now() -> Option<u64> {
            crate::process::wall_clock_ns()
                .ok()
                .map(|ns| ns / 1_000_000_000)
        }
    }

    /// The kernel's random numbers as a `rand_core` generator.
    pub struct KernelRng;

    impl rand_core::RngCore for KernelRng {
        fn next_u32(&mut self) -> u32 {
            let mut b = [0u8; 4];
            self.fill_bytes(&mut b);
            u32::from_le_bytes(b)
        }

        fn next_u64(&mut self) -> u64 {
            let mut b = [0u8; 8];
            self.fill_bytes(&mut b);
            u64::from_le_bytes(b)
        }

        fn fill_bytes(&mut self, dest: &mut [u8]) {
            if crate::sys::random(dest).is_err() {
                // The call cannot fail for a valid buffer; never hand out
                // predictable bytes silently.
                panic!("tls: no random bytes from the kernel");
            }
        }

        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
            self.fill_bytes(dest);
            Ok(())
        }
    }

    impl rand_core::CryptoRng for KernelRng {}

    /// Chooses the root for a chain, then lets `embedded-tls` check it.
    struct Verifier<'a, CS: TlsCipherSuite> {
        roots: &'a Roots,
        host: Option<String>,
        chosen: Option<CertVerifier<'a, CS, WallClock, CERT_SIZE>>,
    }

    impl<CS: TlsCipherSuite> TlsVerifier<CS> for Verifier<'_, CS> {
        fn set_hostname_verification(&mut self, hostname: &str) -> Result<(), TlsError> {
            self.host = Some(String::from(hostname));
            Ok(())
        }

        fn verify_certificate(
            &mut self,
            transcript: &CS::Hash,
            cert: CertificateRef,
        ) -> Result<(), TlsError> {
            // The root is chosen by name: the first certificate of the
            // chain (from the server's own up) whose issuer is a trusted
            // root ends the chain (what follows, e.g. a cross-signed copy
            // of that root, is dropped). embedded-tls then checks the
            // chain with the original message (a rebuilt `CertificateRef`
            // would lose the bytes the later signature check needs).
            // Roots sharing a name (cross-signed variants): the first in
            // the bundle.
            let mut found = None;
            for (i, entry) in cert.entries.iter().enumerate() {
                if let CertificateEntryRef::X509(der) = entry
                    && let Some(root) = self.roots.candidates(der).first()
                {
                    found = Some((i + 1, *root));
                    break;
                }
            }
            let (keep, root) = found.ok_or(TlsError::InvalidCertificate)?;
            let mut cert = cert;
            cert.entries.truncate(keep);
            let mut v = CertVerifier::<CS, WallClock, CERT_SIZE>::new(Certificate::X509(root));
            if let Some(host) = &self.host {
                v.set_hostname_verification(host)?;
            }
            v.verify_certificate(transcript, cert)
                .map_err(|_| TlsError::InvalidCertificate)?;
            self.chosen = Some(v);
            Ok(())
        }

        fn verify_signature(
            &mut self,
            verify: embedded_tls::CertificateVerifyRef,
        ) -> Result<(), TlsError> {
            self.chosen
                .as_mut()
                .ok_or(TlsError::InvalidCertificate)?
                .verify_signature(verify)
        }
    }

    struct Provider<'a> {
        rng: KernelRng,
        verifier: Verifier<'a, Aes128GcmSha256>,
    }

    impl CryptoProvider for Provider<'_> {
        type CipherSuite = Aes128GcmSha256;
        type Signature = Vec<u8>;

        fn rng(&mut self) -> impl embedded_tls::CryptoRngCore {
            &mut self.rng
        }

        fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Self::CipherSuite>, TlsError> {
            Ok(&mut self.verifier)
        }
    }

    /// A STELA TCP socket as an `embedded-io` stream.
    struct Stream {
        socket: Socket,
        timeout_ns: u64,
    }

    /// A socket error for `embedded-io`.
    #[derive(Debug)]
    struct IoError(Error);

    impl core::fmt::Display for IoError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "{:?}", self.0)
        }
    }

    impl core::error::Error for IoError {}

    impl embedded_io::Error for IoError {
        fn kind(&self) -> embedded_io::ErrorKind {
            match self.0 {
                Error::TimedOut => embedded_io::ErrorKind::TimedOut,
                Error::PeerClosed | Error::ConnectionReset => {
                    embedded_io::ErrorKind::ConnectionReset
                }
                _ => embedded_io::ErrorKind::Other,
            }
        }
    }

    impl embedded_io::ErrorType for Stream {
        type Error = IoError;
    }

    impl embedded_io::Read for Stream {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
            self.socket
                .recv(buf, self.timeout_ns)
                .map(|(n, ..)| n)
                .map_err(IoError)
        }
    }

    impl embedded_io::Write for Stream {
        fn write(&mut self, buf: &[u8]) -> Result<usize, IoError> {
            self.socket.send(buf, None).map_err(IoError)
        }

        fn flush(&mut self) -> Result<(), IoError> {
            Ok(())
        }
    }

    /// What went wrong in a TLS exchange, as a STELA error.
    fn tls_error(e: TlsError) -> Error {
        match e {
            TlsError::InvalidCertificate | TlsError::InvalidSignature => Error::AccessDenied,
            TlsError::ConnectionClosed => Error::PeerClosed,
            TlsError::Io(embedded_io::ErrorKind::TimedOut) => Error::TimedOut,
            _ => Error::IoError,
        }
    }

    /// Sends `request` to `host` (`ip`, `port`) over TLS 1.3 and returns
    /// everything the server sent until it closed the connection.
    /// Certificates not signed (through the chain) by one of `roots`, out
    /// of date, or for another name: `AccessDenied`.
    pub fn request(
        host: &str,
        ip: Ip,
        port: u16,
        request: &[u8],
        roots: &Roots,
        timeout_ns: u64,
    ) -> Result<Vec<u8>, Error> {
        let socket = Socket::tcp()?;
        socket.connect(ip, port, timeout_ns)?;
        let mut read_buf = vec![0u8; RECORD_BUFFER];
        let mut write_buf = vec![0u8; RECORD_BUFFER];
        let config = TlsConfig::new().with_server_name(host);
        let mut tls: TlsConnection<'_, Stream, Aes128GcmSha256> =
            TlsConnection::new(Stream { socket, timeout_ns }, &mut read_buf, &mut write_buf);
        let provider = Provider {
            rng: KernelRng,
            verifier: Verifier {
                roots,
                host: None,
                chosen: None,
            },
        };
        tls.open(TlsContext::new(&config, provider))
            .map_err(tls_error)?;
        let mut sent = 0;
        while sent < request.len() {
            sent += tls.write(&request[sent..]).map_err(tls_error)?;
        }
        tls.flush().map_err(tls_error)?;
        let mut out = Vec::new();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            match tls.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                // The server closed: close_notify, or (many servers,
                // Python's among them) just the TCP connection, which
                // embedded-tls reports as an I/O error. HTTP/1.0 with
                // `Connection: close` ends there.
                Err(TlsError::ConnectionClosed) => break,
                Err(TlsError::IoError | TlsError::Io(_)) if !out.is_empty() => break,
                Err(e) => return Err(tls_error(e)),
            }
        }
        Ok(out)
    }
}

#[cfg(target_os = "none")]
pub use client::{KernelRng, WallClock, request};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A tiny certificate-shaped DER: SEQUENCE { SEQUENCE { [0] { INTEGER 2 },
    /// INTEGER 1, SEQUENCE {}, issuer, SEQUENCE {}, subject } }.
    fn fake_cert(issuer: &[u8], subject: &[u8]) -> Vec<u8> {
        let mut tbs = vec![0xA0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x01, 0x01, 0x30, 0x00];
        tbs.extend_from_slice(issuer);
        tbs.extend_from_slice(&[0x30, 0x00]);
        tbs.extend_from_slice(subject);
        let mut inner = vec![0x30, tbs.len() as u8];
        inner.extend_from_slice(&tbs);
        let mut cert = vec![0x30, inner.len() as u8];
        cert.extend_from_slice(&inner);
        cert
    }

    #[test]
    fn der_lengths() {
        assert_eq!(der_element(&[0x30, 0x03, 1, 2, 3]), Some((0x30, 2, 3)));
        let mut long = vec![0x30, 0x82, 0x01, 0x00];
        long.resize(4 + 256, 0);
        assert_eq!(der_element(&long), Some((0x30, 4, 256)));
        assert_eq!(der_element(&[0x30, 0x82, 0x01]), None);
        assert_eq!(der_element(&[0x30, 0x05, 1]), None);
        assert_eq!(der_element(&[0x30, 0x80]), None);
    }

    #[test]
    fn names_and_candidates() {
        let a = [0x30, 0x02, 0x41, 0x41];
        let b = [0x30, 0x02, 0x42, 0x42];
        let c = [0x30, 0x02, 0x43, 0x43];
        let leaf = fake_cert(&b, &c);
        assert_eq!(certificate_names(&leaf), Some((&b[..], &c[..])));
        let root_b = fake_cert(&b, &b);
        let root_a = fake_cert(&a, &a);
        let mut bundle = root_a.clone();
        bundle.extend_from_slice(&root_b);
        let roots = Roots::from_der(bundle);
        assert_eq!(roots.certificates().len(), 2);
        assert_eq!(roots.candidates(&leaf), vec![&root_b[..]]);
        assert!(roots.candidates(&fake_cert(&c, &c)).is_empty());
        assert_eq!(certificate_names(&[0x31, 0x00]), None);
    }
}
