use anyhow::{Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use time::{Duration, OffsetDateTime};

pub struct CaManager {
    pub ca_cert_der: Vec<u8>,
    pub ca_key_der: Vec<u8>,
    pub ca_cert: rcgen::Certificate,
    ca_key: Arc<rcgen::KeyPair>,
    ca_dir: PathBuf,
    validity_days: u32,
}

impl std::fmt::Debug for CaManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaManager")
            .field("ca_cert_der_len", &self.ca_cert_der.len())
            .field("ca_key_der_len", &self.ca_key_der.len())
            .field("ca_dir", &self.ca_dir)
            .field("validity_days", &self.validity_days)
            .finish()
    }
}

impl CaManager {
    pub fn load_or_create(ca_dir: &Path, validity_days: u32) -> Result<Self> {
        let cert_path = ca_dir.join("ca.crt");
        let key_path = ca_dir.join("ca.key");
        if cert_path.exists() && key_path.exists() {
            Self::load_existing(&cert_path, &key_path, ca_dir, validity_days)
        } else {
            Self::generate_new(ca_dir, validity_days)
        }
    }

    fn load_existing(
        cert_path: &Path,
        key_path: &Path,
        ca_dir: &Path,
        validity_days: u32,
    ) -> Result<Self> {
        let cert_pem = fs::read_to_string(cert_path).context("reading CA cert")?;
        let key_pem = fs::read_to_string(key_path).context("reading CA key")?;
        let cert_der = pem::parse(&cert_pem)
            .context("parsing CA cert PEM")?
            .contents
            .to_vec();
        let ca_key = KeyPair::from_pem(&key_pem).context("loading CA key from PEM")?;
        let mut params = CertificateParams::new(vec!["herdr-mcp CA".into()])?;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::OrganizationName, "herdr-mcp");
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "herdr-mcp CA");
        let not_before = OffsetDateTime::now_utc();
        let not_after = not_before + Duration::days(validity_days as i64);
        params.not_before = not_before;
        params.not_after = not_after;
        let ca_cert = params
            .self_signed(&ca_key)
            .context("reconstructing CA cert")?;
        let key_der = ca_key.serialize_der();
        Ok(Self {
            ca_cert_der: cert_der,
            ca_key_der: key_der,
            ca_cert,
            ca_key: Arc::new(ca_key),
            ca_dir: ca_dir.to_path_buf(),
            validity_days,
        })
    }

    fn generate_new(ca_dir: &Path, validity_days: u32) -> Result<Self> {
        fs::create_dir_all(ca_dir).context("creating CA directory")?;
        let mut params = CertificateParams::new(vec!["herdr-mcp CA".into()])?;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::OrganizationName, "herdr-mcp");
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "herdr-mcp CA");
        let not_before = OffsetDateTime::now_utc();
        let not_after = not_before + Duration::days(validity_days as i64);
        params.not_before = not_before;
        params.not_after = not_after;
        let ca_key = KeyPair::generate().context("generating CA key")?;
        let ca_cert = params
            .self_signed(&ca_key)
            .context("self-signing CA cert")?;
        fs::write(ca_dir.join("ca.crt"), ca_cert.pem()).context("writing CA cert")?;
        fs::write(ca_dir.join("ca.key"), ca_key.serialize_pem()).context("writing CA key")?;
        Ok(Self {
            ca_cert_der: ca_cert.der().to_vec(),
            ca_key_der: ca_key.serialize_der(),
            ca_cert,
            ca_key: Arc::new(ca_key),
            ca_dir: ca_dir.to_path_buf(),
            validity_days,
        })
    }

    pub fn ca_cert_pem(&self) -> String {
        self.ca_cert.pem()
    }
    pub fn ca_cert_der(&self) -> &[u8] {
        &self.ca_cert_der
    }

    pub fn sign_leaf(&self, host: &str) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut params = CertificateParams::new(vec![host.into()])?;
        params.is_ca = IsCa::NoCa;
        let not_before = OffsetDateTime::now_utc();
        let not_after = not_before + Duration::days(self.validity_days as i64);
        params.not_before = not_before;
        params.not_after = not_after;
        params.key_usages = vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let leaf_key = KeyPair::generate().context("generating leaf key")?;
        let leaf_cert = params
            .signed_by(&leaf_key, &self.ca_cert, &self.ca_key)
            .context("signing leaf cert")?;
        Ok((leaf_cert.der().to_vec(), leaf_key.serialize_der()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_ca_generate_and_load() {
        let dir = tempdir().unwrap();
        let ca = CaManager::load_or_create(dir.path(), 3650).unwrap();
        assert!(!ca.ca_cert_der.is_empty());
        assert!(!ca.ca_key_der.is_empty());
        assert!(ca.ca_cert_pem().contains("BEGIN CERTIFICATE"));
        let ca2 = CaManager::load_or_create(dir.path(), 3650).unwrap();
        assert_eq!(ca.ca_cert_der, ca2.ca_cert_der);
    }

    #[test]
    fn test_sign_leaf() {
        let dir = tempdir().unwrap();
        let ca = CaManager::load_or_create(dir.path(), 3650).unwrap();
        let (cert_der, key_der) = ca.sign_leaf("api.openai.com").unwrap();
        assert!(!cert_der.is_empty());
        assert!(!key_der.is_empty());
    }

    #[test]
    fn test_sign_leaf_unique_per_call() {
        let dir = tempdir().unwrap();
        let ca = CaManager::load_or_create(dir.path(), 3650).unwrap();
        let (c1, _) = ca.sign_leaf("host.com").unwrap();
        let (c2, _) = ca.sign_leaf("host.com").unwrap();
        // Each sign_leaf generates a fresh keypair; cert DER differs.
        assert_ne!(c1, c2);
        // But the cert length should be in the same ballpark.
        let diff = (c1.len() as i64 - c2.len() as i64).abs();
        assert!(diff < 100, "cert lengths should be similar; got {diff}");
    }

    #[test]
    fn test_sign_leaf_wildcard_host() {
        let dir = tempdir().unwrap();
        let ca = CaManager::load_or_create(dir.path(), 3650).unwrap();
        // Wildcard host should still produce a valid cert.
        let (cert_der, key_der) = ca.sign_leaf("*.anthropic.com").unwrap();
        assert!(!cert_der.is_empty());
        assert!(!key_der.is_empty());
    }
}
