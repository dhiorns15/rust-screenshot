//! FTPS (FTP over TLS), with rustls as RSnip's HTTP is. The server's certificate is checked against
//! the system's trusted authorities, or, for one the user chose to trust (a NAS's self-signed
//! certificate, say), against its SHA-256 fingerprint. File transfers reuse the sign-in's TLS
//! session, which vsftpd and ProFTPD require (Windows' own curl doesn't, so it isn't used). Plain FTP,
//! which would send the password as it is, isn't offered.

use std::net::ToSocketAddrs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use suppaftp::types::FileType;
use suppaftp::{FtpError, RustlsConnector, RustlsFtpStream, Status};

/// A folder on an FTPS server, and how to sign in.
pub struct Server<'a> {
    pub host: &'a str,
    /// 21 for explicit TLS (AUTH TLS), the usual; 990 for implicit.
    pub port: u16,
    pub user: &'a str,
    pub password: &'a str,
    /// "uploads/shots", from the folder the server starts in, or "/srv/www/shots".
    pub folder: &'a str,
    /// The SHA-256 fingerprint of the certificate the user chose to trust; empty to go by the
    /// system's trusted authorities.
    pub trusted: &'a str,
}

/// Signs in and goes to the folder, without changing anything.
pub fn test(s: &Server) -> Result<String, String> {
    let mut ftp = sign_in(s)?;
    let (start, levels) = levels(s.folder);
    if let Some(start) = start {
        ftp.cwd(start).map_err(|e| explain(&e))?;
    }
    for level in &levels {
        if ftp.cwd(level).is_err() {
            let _ = ftp.quit();
            return Ok("Connected. The folder will be made with the first push".into());
        }
    }
    let _ = ftp.quit();
    Ok("Connected".into())
}

/// Uploads `file` into the folder as `name`, making the folder (a level at a time) if need be.
pub fn put(s: &Server, name: &str, file: &Path) -> Result<(), String> {
    let mut ftp = sign_in(s)?;
    // As it is: the default (ASCII) would turn line endings in a PNG into CR LF.
    ftp.transfer_type(FileType::Binary).map_err(|e| explain(&e))?;
    let (start, levels) = levels(s.folder);
    if let Some(start) = start {
        ftp.cwd(start).map_err(|e| explain(&e))?;
    }
    for level in &levels {
        if ftp.cwd(level).is_err() {
            ftp.mkdir(level).and_then(|()| ftp.cwd(level)).map_err(|e| explain(&e))?;
        }
    }
    let mut reader = std::fs::File::open(file).map_err(|e| format!("Couldn't read {name}: {e}"))?;
    ftp.put_file(name, &mut reader).map_err(|e| explain(&e))?;
    let _ = ftp.quit();
    Ok(())
}

/// The SHA-256 fingerprint of the certificate the server shows, for the user to compare with the
/// one the server's settings show before trusting it. Nothing is signed in to.
pub fn certificate(host: &str, port: u16) -> Result<String, String> {
    let seen = Arc::new(Mutex::new(None));
    let verifier = Trust::Note(seen.clone(), provider());
    match connect(host, port, verifier) {
        Ok(ftp) => drop(ftp),
        // Noted before anything else could go wrong.
        Err(_) if seen.lock().is_ok_and(|s| s.is_some()) => {}
        Err(e) => return Err(e),
    }
    let fingerprint = seen.lock().ok().and_then(|s| s.clone());
    fingerprint.ok_or_else(|| "The server didn't show a certificate".into())
}

/// The folder's levels to go into: from the root first if it starts with "/" (`Some("/")`), else
/// from where the server starts.
fn levels(folder: &str) -> (Option<&str>, Vec<&str>) {
    let folder = folder.trim();
    let start = folder.starts_with('/').then_some("/");
    (start, folder.split('/').filter(|p| !p.is_empty()).collect())
}

/// Connects over TLS and signs in.
fn sign_in(s: &Server) -> Result<RustlsFtpStream, String> {
    let trust = if s.trusted.trim().is_empty() { Trust::System(system_verifier()?) } else { Trust::Pinned(s.trusted.trim().to_owned(), provider()) };
    let mut ftp = connect(s.host, s.port, trust)?;
    ftp.login(s.user, s.password).map_err(|e| match e {
        FtpError::UnexpectedResponse(r) if r.status == Status::NotLoggedIn => "The server didn't accept the user name or password".into(),
        e => explain(&e),
    })?;
    Ok(ftp)
}

/// Connects and starts TLS, explicitly (AUTH TLS) or, on port 990, implicitly.
fn connect(host: &str, port: u16, trust: Trust) -> Result<RustlsFtpStream, String> {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    let address = (host, port).to_socket_addrs().ok().and_then(|mut a| a.next()).ok_or("Couldn't find that server: check its name")?;
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(trust))
        .with_no_client_auth();
    let tls = RustlsConnector::from(Arc::new(config));
    let mut ftp = if port == 990 {
        let mut ftp = RustlsFtpStream::connect_secure_implicit(address, tls, host).map_err(|e| explain(&e))?;
        // Transfers encrypted too, as AUTH TLS (below) asks for itself.
        for command in ["PBSZ 0", "PROT P"] {
            ftp.custom_command(command, &[Status::CommandOk]).map_err(|e| explain(&e))?;
        }
        ftp
    } else {
        let plain = RustlsFtpStream::connect_timeout(address, Duration::from_secs(15)).map_err(|e| explain(&e))?;
        plain.into_secure(tls, host).map_err(|e| match e {
            FtpError::UnexpectedResponse(_) => "The server doesn't offer FTP over TLS. RSnip won't send your password unencrypted".into(),
            e => explain(&e),
        })?
    };
    let _ = ftp.get_ref().set_read_timeout(Some(Duration::from_secs(60)));
    // A server behind a router may name its private address for transfers; the one connected to
    // is used instead.
    ftp.set_passive_nat_workaround(true);
    Ok(ftp)
}

/// What went wrong, in words, where it's a usual thing.
fn explain(e: &FtpError) -> String {
    // A certificate turned down comes as a TLS error or, during the handshake, a connection one.
    let tls = match e {
        FtpError::ConnectionError(e) => e.to_string(),
        FtpError::SecureError(e) => e.clone(),
        _ => String::new(),
    };
    match e {
        _ if tls.contains(CHANGED) => {
            "The server's certificate has changed since you trusted it. If you changed it, trust the new one (Edit); if not, something may be in the way".into()
        }
        _ if UNTRUSTED.iter().any(|t| tls.to_ascii_lowercase().contains(t)) => {
            "The server's certificate isn't one this computer trusts (a self-signed one, say): choose Edit, then Trust the server's certificate".into()
        }
        FtpError::ConnectionError(e) => format!("Couldn't connect to the server: check its name and port, and that FTPS is on there ({e})"),
        FtpError::SecureError(e) => format!("Couldn't set up TLS with the server: {e}"),
        FtpError::UnexpectedResponse(r) if matches!(r.status, Status::FileUnavailable | Status::BadFilename) => {
            "The server didn't let RSnip write there: check the folder and that the user may write to it".into()
        }
        FtpError::UnexpectedResponse(r) => format!("The server said: {}", r.as_string().unwrap_or_default().trim()),
        e => e.to_string(),
    }
}

const CHANGED: &str = "not the certificate that was trusted";
/// How rustls says a certificate isn't trusted (in lower case): by no authority the system knows,
/// for another name, expired, or an authority's own.
const UNTRUSTED: [&str; 6] = ["unknownissuer", "unknown issuer", "not valid for name", "notvalidforname", "expired", "causedasendentity"];

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn system_verifier() -> Result<rustls_platform_verifier::Verifier, String> {
    rustls_platform_verifier::Verifier::new(provider()).map_err(|e| format!("Couldn't read this computer's trusted certificates: {e}"))
}

/// `AB:CD:…`: the SHA-256 of a certificate, as servers' settings show it.
fn fingerprint(certificate: &[u8]) -> String {
    Sha256::digest(certificate).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// How the server's certificate is checked.
#[derive(Debug)]
enum Trust {
    /// By the system's trusted authorities.
    System(rustls_platform_verifier::Verifier),
    /// It's the one with this fingerprint (whoever signed it, whatever name it's for).
    Pinned(String, Arc<CryptoProvider>),
    /// Any, noting its fingerprint: only to show it, never to sign in.
    Note(Arc<Mutex<Option<String>>>, Arc<CryptoProvider>),
}

impl Trust {
    fn provider(&self) -> Option<&CryptoProvider> {
        match self {
            Trust::System(_) => None,
            Trust::Pinned(_, p) | Trust::Note(_, p) => Some(p),
        }
    }
}

impl ServerCertVerifier for Trust {
    fn verify_server_cert(&self, end: &CertificateDer<'_>, others: &[CertificateDer<'_>], name: &ServerName<'_>, ocsp: &[u8], now: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        match self {
            Trust::System(v) => v.verify_server_cert(end, others, name, ocsp, now),
            Trust::Pinned(trusted, _) if fingerprint(end).eq_ignore_ascii_case(trusted) => Ok(ServerCertVerified::assertion()),
            Trust::Pinned(..) => Err(rustls::Error::General(CHANGED.into())),
            Trust::Note(seen, _) => {
                if let Ok(mut seen) = seen.lock() {
                    *seen = Some(fingerprint(end));
                }
                Ok(ServerCertVerified::assertion())
            }
        }
    }

    // The server still has to prove it holds the certificate's key.
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        match (self, self.provider()) {
            (Trust::System(v), _) => v.verify_tls12_signature(message, cert, dss),
            (_, Some(p)) => rustls::crypto::verify_tls12_signature(message, cert, dss, &p.signature_verification_algorithms),
            _ => unreachable!(),
        }
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        match (self, self.provider()) {
            (Trust::System(v), _) => v.verify_tls13_signature(message, cert, dss),
            (_, Some(p)) => rustls::crypto::verify_tls13_signature(message, cert, dss, &p.signature_verification_algorithms),
            _ => unreachable!(),
        }
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        match (self, self.provider()) {
            (Trust::System(v), _) => v.supported_verify_schemes(),
            (_, Some(p)) => p.signature_verification_algorithms.supported_schemes(),
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_start_where_the_server_does_unless_absolute() {
        assert_eq!(levels("shots/today/"), (None, vec!["shots", "today"]));
        assert_eq!(levels(" /srv/www "), (Some("/"), vec!["srv", "www"]));
        assert_eq!(levels(""), (None, vec![]));
    }

    #[test]
    fn fingerprints_read_as_servers_show_them() {
        // SHA-256 of "abc" (FIPS 180-2).
        assert_eq!(fingerprint(b"abc"), "BA:78:16:BF:8F:01:CF:EA:41:41:40:DE:5D:AE:22:23:B0:03:61:A3:96:17:7A:9C:B4:10:FF:61:F2:00:15:AD");
        let pinned = Trust::Pinned(fingerprint(b"abc").to_lowercase(), provider());
        let name = ServerName::try_from("nas").unwrap();
        let now = UnixTime::now();
        assert!(pinned.verify_server_cert(&CertificateDer::from(&b"abc"[..]), &[], &name, &[], now).is_ok(), "case doesn't matter");
        let error = pinned.verify_server_cert(&CertificateDer::from(&b"abd"[..]), &[], &name, &[], now).unwrap_err();
        assert!(explain(&FtpError::SecureError(error.to_string())).starts_with("The server's certificate has changed"));
    }

    #[test]
    fn explains_the_usual_failures() {
        for untrusted in ["invalid peer certificate: UnknownIssuer", "invalid peer certificate: certificate not valid for name \"10.0.0.5\""] {
            assert!(explain(&FtpError::SecureError(untrusted.into())).contains("Trust the server's certificate"), "{untrusted}");
        }
        assert!(explain(&FtpError::ConnectionError(std::io::ErrorKind::ConnectionRefused.into())).starts_with("Couldn't connect"));
    }
}
