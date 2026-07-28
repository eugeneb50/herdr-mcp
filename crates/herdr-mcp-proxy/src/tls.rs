use crate::ca::CaManager;
use anyhow::Result;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::ResolvesServerCert;
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

struct ResolverInner {
    ca: Arc<CaManager>,
    target_hosts: Vec<String>,
    cache: HashMap<String, Arc<CertifiedKey>>,
}

pub struct SniResolver {
    inner: RwLock<ResolverInner>,
}

impl std::fmt::Debug for SniResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.read().unwrap();
        f.debug_struct("SniResolver")
            .field("target_hosts", &inner.target_hosts)
            .field("cache_size", &inner.cache.len())
            .finish()
    }
}

impl SniResolver {
    pub fn new(ca: Arc<CaManager>, target_hosts: Vec<String>) -> Self {
        Self {
            inner: RwLock::new(ResolverInner {
                ca,
                target_hosts,
                cache: HashMap::new(),
            }),
        }
    }

    fn is_allowed(&self, host: &str) -> bool {
        let inner = self.inner.read().unwrap();
        inner.target_hosts.iter().any(|allowed| {
            if allowed == host {
                return true;
            }
            if let Some(suffix) = allowed.strip_prefix("*.") {
                host.ends_with(suffix)
                    && host != suffix
                    && host.chars().nth(host.len() - suffix.len() - 1) == Some('.')
            } else {
                false
            }
        })
    }

    fn get_or_create(&self, host: &str) -> Result<Arc<CertifiedKey>> {
        {
            let inner = self.inner.read().unwrap();
            if let Some(cached) = inner.cache.get(host) {
                return Ok(cached.clone());
            }
        }
        if !self.is_allowed(host) {
            anyhow::bail!("Host '{}' not in allowed target list", host);
        }
        let (cert_der, key_der) = {
            let inner = self.inner.read().unwrap();
            inner.ca.sign_leaf(host)?
        };
        let cert = CertificateDer::from(cert_der);
        let key = PrivateKeyDer::try_from(key_der)
            .map_err(|e| anyhow::anyhow!("invalid private key DER: {e}"))?;
        let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)
            .map_err(|e| anyhow::anyhow!("failed to create signing key: {e}"))?;
        let certified_key = Arc::new(CertifiedKey::new(vec![cert], signing_key));
        {
            let mut inner = self.inner.write().unwrap();
            inner.cache.insert(host.to_string(), certified_key.clone());
        }
        Ok(certified_key)
    }
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, client_hello: rustls::server::ClientHello) -> Option<Arc<CertifiedKey>> {
        let host = client_hello.server_name()?;
        let host = host.to_string();
        tracing::debug!("SNI resolve: {host}");
        self.get_or_create(&host).ok()
    }
}

pub fn make_server_config(ca: Arc<CaManager>, target_hosts: Vec<String>) -> Result<ServerConfig> {
    let resolver = Arc::new(SniResolver::new(ca, target_hosts));
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_sni_resolver_allowed() {
        let dir = tempdir().unwrap();
        let ca = Arc::new(CaManager::load_or_create(dir.path(), 3650).unwrap());
        let resolver =
            SniResolver::new(ca, vec!["api.openai.com".into(), "*.anthropic.com".into()]);
        assert!(resolver.is_allowed("api.openai.com"));
        assert!(resolver.is_allowed("api.anthropic.com"));
        assert!(resolver.is_allowed("console.anthropic.com"));
        assert!(!resolver.is_allowed("evil.com"));
        assert!(!resolver.is_allowed("anthropic.com"));
    }

    #[test]
    fn test_sni_resolver_caches_cert() {
        let dir = tempdir().unwrap();
        let ca = Arc::new(CaManager::load_or_create(dir.path(), 3650).unwrap());
        let resolver = SniResolver::new(ca, vec!["api.openai.com".into()]);
        let k1 = resolver.get_or_create("api.openai.com").unwrap();
        let k2 = resolver.get_or_create("api.openai.com").unwrap();
        assert!(Arc::ptr_eq(&k1, &k2), "second call should return cached");
    }

    #[test]
    fn test_sni_resolver_rejects_disallowed_host() {
        let dir = tempdir().unwrap();
        let ca = Arc::new(CaManager::load_or_create(dir.path(), 3650).unwrap());
        let resolver = SniResolver::new(ca, vec!["api.openai.com".into()]);
        let result = resolver.get_or_create("evil.com");
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("not in allowed"),
            "error should mention allowed list"
        );
    }

    #[test]
    fn test_make_server_config_ok() {
        let dir = tempdir().unwrap();
        let ca = Arc::new(CaManager::load_or_create(dir.path(), 3650).unwrap());
        let config = make_server_config(ca, vec!["api.openai.com".into()]).unwrap();
        // ServerConfig is built; we can only verify it doesn't error
        drop(config);
    }

    #[test]
    fn test_sni_wildcard_multi_level_subdomains() {
        // Documenting observed behavior: `*.example.com` ends-with matches any
        // host whose tail is `.example.com`, regardless of subdomain depth.
        let dir = tempdir().unwrap();
        let ca = Arc::new(CaManager::load_or_create(dir.path(), 3650).unwrap());
        let resolver = SniResolver::new(ca, vec!["*.example.com".into()]);
        assert!(resolver.is_allowed("a.example.com"));
        assert!(resolver.is_allowed("deep.sub.example.com"));
        assert!(!resolver.is_allowed("example.com"));
        assert!(!resolver.is_allowed("notexample.com"));
        assert!(!resolver.is_allowed("evil.com"));
    }
}
