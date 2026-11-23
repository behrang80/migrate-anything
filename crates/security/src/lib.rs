//! Security primitives. Uses audited libraries only (RustCrypto AEAD/HMAC, rustls); no custom
//! cryptography.

pub mod approval;
pub mod keys;
pub mod network;
pub mod redact;
pub mod secrets;

/// Random hex string from the OS CSPRNG.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("OS random number generator unavailable");
    hex::encode(buf)
}

/// Operator identity as reported by the operating system.
pub fn os_operator() -> String {
    std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "unknown-operator".to_string())
}
