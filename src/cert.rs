//! The local root certificate the proxy signs its two hosts' certificates with.
//!
//! `mmr cert new` makes it on this machine. Like Fiddler's DO_NOT_TRUST_FiddlerRoot,
//! its name marks it as a local test certificate not to trust anywhere else. Critical
//! name constraints limit it to the two hosts the proxy reads and exclude every IP
//! address, so a copy can't vouch for any other site. Its key is encrypted with DPAPI
//! for the current user, so a copied key file is useless on another machine or account.
//! It goes in the current user's Root store, where Windows asks before adding or
//! removing it.

use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
    GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints, SerialNumber,
};
use ring::rand::{SecureRandom, SystemRandom};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use time::{Duration, OffsetDateTime};

/// The only hosts the proxy decrypts, and the only names the root can vouch for.
pub const HOSTS: [&str; 2] = ["lobby-hi.svc.halowaypoint.com", "sessiondirectory.xboxlive.com"];
/// Part of the name of every root mmr makes, used to find them again.
pub const MARK: &str = "DO_NOT_TRUST_mmr";
const ROOT_DAYS: i64 = 365;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct About {
    pub subject: String,
    pub sha1: String,
    pub created: String,
    pub expires: String,
    pub expires_unix: i64,
}

pub struct Root {
    pub der: Vec<u8>,
    pub about: About,
    key: KeyPair,
}

/// Where a store keeps the root's files.
pub(crate) fn cert_dir(store: &Path) -> PathBuf {
    store.join("cert")
}

fn files(store: &Path) -> [PathBuf; 3] {
    let d = cert_dir(store);
    [d.join("root.cer"), d.join("root.key"), d.join("root.json")]
}

fn machine() -> String {
    let m: String = std::env::var("COMPUTERNAME")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(15)
        .collect();
    if m.is_empty() {
        "this-machine".to_string()
    } else {
        m
    }
}

fn serial() -> Result<SerialNumber, String> {
    let mut b = [0u8; 16];
    SystemRandom::new().fill(&mut b).map_err(|_| "no random numbers")?;
    b[0] = (b[0] & 0x7f) | 0x40;
    Ok(SerialNumber::from_slice(&b))
}

#[expect(clippy::format_collect, reason = "20 bytes, so speed doesn't matter")]
pub fn sha1_hex(der: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, der)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()))
}

fn write(path: &Path, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|e| format!("can't write {}: {e}", path.display()))
}

impl Root {
    /// A new root, not yet saved or installed.
    pub fn make() -> Result<Root, String> {
        let key = KeyPair::generate().map_err(err)?;
        let now = OffsetDateTime::now_utc();
        let m = machine();
        let subject = format!("{MARK}_LocalRoot_{m}");
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, subject.clone());
        dn.push(DnType::OrganizationName, "DO_NOT_TRUST - local test certificate for this machine only");
        dn.push(DnType::OrganizationalUnitName, format!("Made by mmr on {m}"));
        let mut p = CertificateParams::default();
        p.distinguished_name = dn;
        p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
        // rcgen marks the extension critical.
        p.name_constraints = Some(NameConstraints {
            permitted_subtrees: HOSTS.iter().map(|h| GeneralSubtree::DnsName(h.to_string())).collect(),
            excluded_subtrees: vec![
                GeneralSubtree::IpAddress(CidrSubnet::V4([0; 4], [0; 4])),
                GeneralSubtree::IpAddress(CidrSubnet::V6([0; 16], [0; 16])),
            ],
        });
        p.not_before = now - Duration::days(1);
        p.not_after = now + Duration::days(ROOT_DAYS);
        p.serial_number = Some(serial()?);
        let der = p.self_signed(&key).map_err(err)?.der().to_vec();
        let about = About {
            subject,
            sha1: sha1_hex(&der),
            created: crate::clock::iso(now),
            expires: crate::clock::iso(p.not_after),
            expires_unix: p.not_after.unix_timestamp(),
        };
        Ok(Root { der, about, key })
    }

    /// Writes the certificate, its description and its DPAPI-protected key.
    pub fn save(&self, store: &Path) -> Result<(), String> {
        let [cer, key, json] = files(store);
        let dir = cert_dir(store);
        std::fs::create_dir_all(&dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
        let sealed = dpapi::protect(&self.key.serialize_der())?;
        write(&key, &sealed)?;
        write(&cer, &self.der)?;
        write(&json, &serde_json::to_vec_pretty(&self.about).map_err(err)?)
    }

    /// The saved root, or `None` if there isn't one.
    pub fn load(store: &Path) -> Result<Option<Root>, String> {
        let [cer, key, json] = files(store);
        if !cer.exists() || !key.exists() {
            return Ok(None);
        }
        let der = read(&cer)?;
        let pkcs8 = dpapi::unprotect(&read(&key)?).map_err(|e| {
            format!("the root's key can't be opened by this user on this machine ({e}): run `mmr cert new`")
        })?;
        let key = KeyPair::try_from(pkcs8.as_slice()).map_err(err)?;
        let public = key.public_key_raw();
        if !der.windows(public.len()).any(|w| w == public) {
            return Err("the saved key isn't the root's: run `mmr cert new`".into());
        }
        let about: About =
            serde_json::from_slice(&read(&json)?).map_err(|e| format!("can't read {}: {e}", json.display()))?;
        // A save that failed halfway can leave another root's description.
        if sha1_hex(&der) != about.sha1 {
            return Err(format!("{} doesn't describe the saved root: run `mmr cert new`", json.display()));
        }
        Ok(Some(Root { der, about, key }))
    }

    /// Deletes the saved files, and their folder if nothing else is in it.
    pub fn forget(store: &Path) -> usize {
        let n = files(store).iter().filter(|f| std::fs::remove_file(f).is_ok()).count();
        let _ = std::fs::remove_dir(cert_dir(store));
        n
    }

    pub fn days_left(&self) -> i64 {
        (self.about.expires_unix - OffsetDateTime::now_utc().unix_timestamp()) / 86_400
    }

    /// A server certificate for `host`, with its key. It's made once a run, and a
    /// watch can run for days, so it lasts until the root expires.
    pub fn leaf(&self, host: &str) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>), String> {
        let ca = CertificateDer::from(self.der.as_slice());
        let issuer = Issuer::from_ca_cert_der(&ca, &self.key).map_err(err)?;
        let key = KeyPair::generate().map_err(err)?;
        let now = OffsetDateTime::now_utc();
        let mut p = CertificateParams::new(vec![host.to_string()]).map_err(err)?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, host);
        p.distinguished_name = dn;
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        p.use_authority_key_identifier_extension = true;
        p.not_before = now - Duration::days(1);
        p.not_after = OffsetDateTime::from_unix_timestamp(self.about.expires_unix).map_err(err)?;
        p.serial_number = Some(serial()?);
        let cert = p.signed_by(&key, &issuer).map_err(err)?;
        Ok((cert.der().clone(), PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()))))
    }
}

/// The key, sealed for the current user on this machine.
mod dpapi {
    use std::io;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    const ENTROPY: &[u8] = b"mmr local root key";

    fn blob(b: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: b.len() as u32, pbData: b.as_ptr().cast_mut() }
    }

    /// Copies what a DPAPI call wrote to `out`, then frees it.
    ///
    /// # Safety
    ///
    /// `out` follows a successful call, so `pbData` is from LocalAlloc with `cbData`
    /// bytes, and isn't used after.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        // SAFETY: pbData holds cbData bytes, as the caller says.
        let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
        // SAFETY: from LocalAlloc, and not used after.
        unsafe { LocalFree(out.pbData.cast()) };
        v
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        let description = crate::wide("mmr local root key");
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: the input blobs and the description point into live slices that are
        // only read, and out is written.
        let ok = unsafe {
            CryptProtectData(
                &blob(plain),
                description.as_ptr(),
                &blob(ENTROPY),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut out,
            )
        };
        if ok == 0 {
            return Err(format!("DPAPI couldn't protect the key: {}", io::Error::last_os_error()));
        }
        // SAFETY: the call succeeded.
        Ok(unsafe { take(out) })
    }

    pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: the input blobs point into live slices that are only read, and out is
        // written.
        let ok = unsafe {
            CryptUnprotectData(
                &blob(sealed),
                std::ptr::null_mut(),
                &blob(ENTROPY),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut out,
            )
        };
        if ok == 0 {
            return Err(format!("DPAPI: {}", io::Error::last_os_error()));
        }
        // SAFETY: the call succeeded.
        Ok(unsafe { take(out) })
    }
}

/// The current user's Root store.
pub mod store {
    use super::{sha1_hex, MARK};
    use std::io;
    use windows_sys::Win32::Foundation::ERROR_CANCELLED;
    use windows_sys::Win32::Security::Cryptography::{
        CertAddEncodedCertificateToStore, CertCloseStore, CertDeleteCertificateFromStore, CertFindCertificateInStore,
        CertOpenStore, CERT_CONTEXT, CERT_FIND_SHA1_HASH, CERT_FIND_SUBJECT_STR_W, CERT_STORE_ADD_REPLACE_EXISTING,
        CERT_STORE_PROV_SYSTEM_W, CERT_SYSTEM_STORE_CURRENT_USER, CRYPT_INTEGER_BLOB, HCERTSTORE, PKCS_7_ASN_ENCODING,
        X509_ASN_ENCODING,
    };

    const ENCODING: u32 = X509_ASN_ENCODING | PKCS_7_ASN_ENCODING;

    struct Store(HCERTSTORE);

    impl Drop for Store {
        fn drop(&mut self) {
            // SAFETY: the value owns the non-null store, and closes it only here.
            unsafe { CertCloseStore(self.0, 0) };
        }
    }

    fn open() -> Result<Store, String> {
        let name = crate::wide("Root");
        // SAFETY: the name is nul-terminated, and outlives the call.
        let h = unsafe {
            CertOpenStore(CERT_STORE_PROV_SYSTEM_W, 0, 0, CERT_SYSTEM_STORE_CURRENT_USER, name.as_ptr().cast())
        };
        if h.is_null() {
            return Err(format!("can't open the Root store: {}", io::Error::last_os_error()));
        }
        Ok(Store(h))
    }

    /// Why a change failed, from the error the call left.
    fn failed(what: &str, e: &io::Error) -> String {
        if e.raw_os_error() == Some(ERROR_CANCELLED.cast_signed()) {
            format!("{what}: declined in the Windows prompt")
        } else {
            format!("{what}: {e}")
        }
    }

    /// Adds the certificate. Windows asks the user first.
    pub fn add(der: &[u8]) -> Result<(), String> {
        let s = open()?;
        // SAFETY: der is valid for its length.
        let ok = unsafe {
            CertAddEncodedCertificateToStore(
                s.0,
                ENCODING,
                der.as_ptr(),
                der.len() as u32,
                CERT_STORE_ADD_REPLACE_EXISTING,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(failed("the certificate wasn't added to your Root store", &io::Error::last_os_error()));
        }
        Ok(())
    }

    /// Every root mmr made that is in the store, as DER.
    pub fn ours() -> Result<Vec<Vec<u8>>, String> {
        let s = open()?;
        let mark = crate::wide(MARK);
        let mut out = Vec::new();
        let mut prev: *const CERT_CONTEXT = std::ptr::null();
        loop {
            // SAFETY: prev is null or the context the previous call returned, which this
            // call frees, and the mark is nul-terminated and outlives the call.
            let c = unsafe {
                CertFindCertificateInStore(s.0, ENCODING, 0, CERT_FIND_SUBJECT_STR_W, mark.as_ptr().cast(), prev)
            };
            if c.is_null() {
                break;
            }
            // SAFETY: c lives until the next find frees it, and pbCertEncoded holds
            // cbCertEncoded bytes, copied before then.
            let der = unsafe { std::slice::from_raw_parts((*c).pbCertEncoded, (*c).cbCertEncoded as usize) };
            out.push(der.to_vec());
            prev = c;
        }
        Ok(out)
    }

    pub fn has(der: &[u8]) -> Result<bool, String> {
        let want = sha1_hex(der);
        Ok(ours()?.iter().any(|d| sha1_hex(d) == want))
    }

    /// Removes one certificate. Windows asks the user first.
    pub fn remove(der: &[u8]) -> Result<(), String> {
        let s = open()?;
        let hash = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, der);
        let digest = hash.as_ref();
        let blob = CRYPT_INTEGER_BLOB { cbData: digest.len() as u32, pbData: digest.as_ptr().cast_mut() };
        // SAFETY: the blob points at the digest, which is alive for the call.
        let c = unsafe {
            CertFindCertificateInStore(
                s.0,
                ENCODING,
                0,
                CERT_FIND_SHA1_HASH,
                (&raw const blob).cast(),
                std::ptr::null(),
            )
        };
        if c.is_null() {
            return Ok(());
        }
        // SAFETY: the context was just found, and the call frees it, whether or not it
        // succeeds.
        if unsafe { CertDeleteCertificateFromStore(c) } == 0 {
            return Err(failed("the certificate wasn't removed from your Root store", &io::Error::last_os_error()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_is_constrained_and_signs_its_hosts() {
        let root = Root::make().unwrap();
        // The name constraints extension (2.5.29.30), marked critical (01 01 ff).
        let nc = [0x06, 0x03, 0x55, 0x1d, 0x1e, 0x01, 0x01, 0xff];
        assert!(root.der.windows(nc.len()).any(|w| w == nc));
        for h in HOSTS {
            assert!(root.der.windows(h.len()).any(|w| w == h.as_bytes()));
        }
        assert!(root.about.subject.starts_with(MARK));
        let (leaf, _) = root.leaf(HOSTS[0]).unwrap();
        assert!(leaf.windows(HOSTS[0].len()).any(|w| w == HOSTS[0].as_bytes()));
    }

    /// A verifier that trusts only the root.
    fn verifier(root: &Root) -> std::sync::Arc<rustls::client::WebPkiServerVerifier> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(root.der.clone())).unwrap();
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        rustls::client::WebPkiServerVerifier::builder_with_provider(std::sync::Arc::new(roots), provider)
            .build()
            .unwrap()
    }

    /// A verifier that trusts the root accepts its two hosts and nothing else, even a
    /// certificate the root itself signed.
    #[test]
    fn the_name_constraints_hold_when_verifying() {
        use rustls::client::danger::ServerCertVerifier;
        use rustls::pki_types::{ServerName, UnixTime};
        let root = Root::make().unwrap();
        let verifier = verifier(&root);
        let check = |host: &str| {
            let (leaf, _) = root.leaf(host).unwrap();
            let name = ServerName::try_from(host.to_string()).unwrap();
            verifier.verify_server_cert(&leaf, &[], &name, &[], UnixTime::now())
        };
        for h in HOSTS {
            check(h).unwrap();
        }
        assert!(check("example.com").is_err());
        assert!(check("www.halowaypoint.com").is_err());
    }

    /// A leaf is made once a run, so it lasts as long as the root.
    #[test]
    fn a_leaf_lasts_until_the_root_expires() {
        use rustls::client::danger::ServerCertVerifier;
        use rustls::pki_types::{ServerName, UnixTime};
        let root = Root::make().unwrap();
        let verifier = verifier(&root);
        let (leaf, _) = root.leaf(HOSTS[0]).unwrap();
        let name = ServerName::try_from(HOSTS[0]).unwrap();
        let after = |days: i64| {
            let t = (OffsetDateTime::now_utc() + Duration::days(days)).unix_timestamp();
            UnixTime::since_unix_epoch(std::time::Duration::from_secs(u64::try_from(t).unwrap()))
        };
        verifier.verify_server_cert(&leaf, &[], &name, &[], after(ROOT_DAYS - 1)).unwrap();
        assert!(verifier.verify_server_cert(&leaf, &[], &name, &[], after(ROOT_DAYS + 1)).is_err());
    }

    /// Saves to the test's own folder, never the real store.
    #[test]
    fn a_saved_root_loads_only_with_its_own_description() {
        let store = std::env::temp_dir().join(format!("mmr-test-cert-{}", std::process::id()));
        let root = Root::make().unwrap();
        root.save(&store).unwrap();
        assert_eq!(Root::load(&store).unwrap().unwrap().about.sha1, root.about.sha1);
        let [_, _, json] = files(&store);
        let other = About { sha1: "00".repeat(20), ..root.about };
        std::fs::write(&json, serde_json::to_vec(&other).unwrap()).unwrap();
        let Err(e) = Root::load(&store) else { panic!("a mismatched description loaded") };
        assert!(e.contains("doesn't describe the saved root"), "{e}");
        std::fs::remove_file(&json).unwrap();
        let Err(e) = Root::load(&store) else { panic!("a root without a description loaded") };
        assert!(e.contains(&json.display().to_string()), "{e}");
        assert_eq!(Root::forget(&store), 2);
        assert!(!cert_dir(&store).exists());
        std::fs::remove_dir(&store).unwrap();
    }

    #[test]
    fn the_key_round_trips_through_dpapi() {
        let sealed = dpapi::protect(b"secret").unwrap();
        assert!(!sealed.windows(6).any(|w| w == b"secret"));
        assert_eq!(dpapi::unprotect(&sealed).unwrap(), b"secret");
    }
}
