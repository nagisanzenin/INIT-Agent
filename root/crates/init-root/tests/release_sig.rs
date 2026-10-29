//! v0.5.3 (PLAN-052 L4; docs/release-signing.md): `init-root release verify` and `release keys` through the binary,
//! as get.sh `--upgrade` runs them on the installed Root: the exit codes get.sh reads (0 verified, 5 only expired,
//! 3 refused, 64 usage), the JSON it parses, and the downgrade check against the install.json of this ROOT_HOME.
//! Signatures come from throwaway keys made at run time (/dev/urandom, or `ssh-keygen` for the case that checks
//! OpenSSH's own output), given to this test build through `INIT_TEST_RELEASE_KEYS` (honoured by `test-fixtures`
//! builds only; a release build pins its key in the binary). Nothing is committed and nothing outside
//! `root/target/test-homes/rsig/` is written.
//!
//! Run with `cargo test --offline -p init-root --features test-fixtures --test release_sig`.
#![cfg(feature = "test-fixtures")]

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_init-root");

struct Home(PathBuf);
impl Drop for Home {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = fs::remove_dir_all(&self.0); // exact path, made by home()
        }
    }
}

fn home(name: &str) -> Home {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-homes/rsig");
    fs::create_dir_all(&base).unwrap();
    let d = fs::canonicalize(base).unwrap().join(name);
    if d.exists() {
        fs::remove_dir_all(&d).unwrap(); // exact path, from an earlier run of this test
    }
    fs::create_dir_all(d.join("rh")).unwrap();
    Home(d)
}

fn wire(fields: &[&[u8]]) -> Vec<u8> {
    fields.iter().flat_map(|f| (f.len() as u32).to_be_bytes().into_iter().chain(f.iter().copied())).collect()
}

/// A throwaway key and its OpenSSH `.pub` line.
fn key() -> (SigningKey, String) {
    let mut seed = [0u8; 32];
    fs::File::open("/dev/urandom").unwrap().read_exact(&mut seed).unwrap();
    let sk = SigningKey::from_bytes(&seed);
    let line = format!("ssh-ed25519 {} INIT TEST KEY", STANDARD.encode(wire(&[b"ssh-ed25519", sk.verifying_key().as_bytes()])));
    (sk, line)
}

/// `ssh-keygen -Y sign -n init-release`'s armored SSHSIG of `msg`.
fn sign(sk: &SigningKey, msg: &[u8]) -> String {
    let pk = wire(&[b"ssh-ed25519", sk.verifying_key().as_bytes()]);
    let signed = [b"SSHSIG".as_slice(), &wire(&[b"init-release", b"", b"sha512", &Sha512::digest(msg)])].concat();
    let sig = wire(&[b"ssh-ed25519", &sk.sign(&signed).to_bytes()]);
    let blob = [b"SSHSIG".as_slice(), &[0, 0, 0, 1], &wire(&[&pk, b"init-release", b"", b"sha512", &sig])].concat();
    format!("-----BEGIN SSH SIGNATURE-----\n{}\n-----END SSH SIGNATURE-----\n", STANDARD.encode(blob))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn date(days_from_today: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let z = (now / 86_400 + days_from_today) + 719_468; // civil-from-days (H. Hinnant)
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + i64::from(m <= 2))
}

const BUNDLE: &str = "init-v0.5.4-preview-macos-arm64.tar.gz";

impl Home {
    /// The release files: the bundle, SHA256SUMS for `version` until `expires`, signed by `sk`.
    fn release(&self, version: &str, expires: &str, sk: &SigningKey) {
        let data = b"a stand-in bundle\n";
        fs::write(self.0.join(BUNDLE), data).unwrap();
        let sums = format!("# INIT release SHA256SUMS v1\n# version: {version}\n# expires: {expires}\n{}  {BUNDLE}\n", hex(&Sha256::digest(data)));
        fs::write(self.0.join("SHA256SUMS"), &sums).unwrap();
        fs::write(self.0.join("SHA256SUMS.sig"), sign(sk, sums.as_bytes())).unwrap();
    }
    fn installed(&self, version: &str) {
        fs::write(self.0.join("rh/install.json"), json!({"release": {"commit": "", "version": version}, "v": 1}).to_string()).unwrap();
    }
    fn run(&self, keys: &str, args: &[&str]) -> Output {
        Command::new(BIN).env_clear().env("HOME", &self.0).env("PATH", "/usr/bin:/bin").env("INIT_TEST_RELEASE_KEYS", keys)
            .arg("--root-home").arg(self.0.join("rh")).args(args).stdin(Stdio::null()).output().unwrap()
    }
    fn verify(&self, keys: &str, extra: &[&str]) -> (Option<i32>, Value, String) {
        let (s, g, b) = (self.0.join("SHA256SUMS"), self.0.join("SHA256SUMS.sig"), self.0.join(BUNDLE));
        let mut a = vec!["release", "verify", "--sums", s.to_str().unwrap(), "--sig", g.to_str().unwrap(), "--file", b.to_str().unwrap()];
        a.extend(extra);
        let o = self.run(keys, &a);
        (o.status.code(), serde_json::from_slice(&o.stdout).unwrap_or(Value::Null), String::from_utf8_lossy(&o.stderr).into_owned())
    }
}

#[test]
fn verify_answers_get_sh_with_its_exit_codes_and_json() {
    let h = home("codes");
    let ((sk, pub1), (other, _)) = (key(), key());
    h.installed("0.5.3-preview");
    h.release("0.5.4-preview", &date(180), &sk);
    let (code, v, err) = h.verify(&pub1, &[]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!((v["state"].as_str(), v["version"].as_str(), v["installed"].as_str()), (Some("newer"), Some("0.5.4-preview"), Some("0.5.3-preview")));
    assert_eq!(v["file"], json!({"name": BUNDLE, "sha256": hex(&Sha256::digest(b"a stand-in bundle\n"))}));
    assert!(v["key"].as_str().is_some_and(|k| k.starts_with("SHA256:")), "{v}");
    // the bundle swapped after the signature: refused (3)
    fs::write(h.0.join(BUNDLE), b"another bundle\n").unwrap();
    let (code, _, err) = h.verify(&pub1, &[]);
    assert!(code == Some(3) && err.contains("is not the file the maintainer signed"), "{code:?} {err}");
    // signed by a key this Root does not pin
    h.release("0.5.4-preview", &date(180), &other);
    let (code, _, err) = h.verify(&pub1, &[]);
    assert!(code == Some(3) && err.contains("a key this INIT does not trust"), "{code:?} {err}");
    // older than the installed release: refused, with or without --allow-expired
    h.release("0.5.2", &date(180), &sk);
    for extra in [&[][..], &["--allow-expired"][..]] {
        let (code, _, err) = h.verify(&pub1, extra);
        assert!(code == Some(3) && err.contains("older than the installed 0.5.3-preview: INIT does not downgrade"), "{code:?} {err}");
    }
    // expired: 5, unless --allow-expired; the installed release expired: 0 with a note
    h.release("0.5.4-preview", &date(-1), &sk);
    let (code, _, err) = h.verify(&pub1, &[]);
    assert!(code == Some(5) && err.contains(&format!("expired on {}", date(-1))), "{code:?} {err}");
    let (code, v, _) = h.verify(&pub1, &["--allow-expired"]);
    assert_eq!((code, v["expired"].as_bool()), (Some(0), Some(true)));
    h.installed("0.5.4-preview");
    let (code, v, err) = h.verify(&pub1, &[]);
    assert_eq!((code, v["state"].as_str()), (Some(0), Some("current")), "{err}");
    assert!(err.contains("is the release installed here, and its signature expired"), "{err}");
    // no key pinned (a development build): nothing verifies; no signature file: an error, not a pass
    let (code, _, err) = h.verify("", &[]);
    assert!(code == Some(3) && err.contains("pins no release key"), "{code:?} {err}");
    fs::remove_file(h.0.join("SHA256SUMS.sig")).unwrap();
    assert_eq!(h.verify(&pub1, &[]).0, Some(1));
    assert_eq!(h.run(&pub1, &["release", "verify", "--sums", "x"]).status.code(), Some(64));
}

#[test]
fn keys_lists_the_pinned_fingerprints() {
    let h = home("keys");
    let (_, pub1) = key();
    let o = h.run(&pub1, &["release", "keys", "--json"]);
    assert_eq!(o.status.code(), Some(0));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let blob = STANDARD.decode(pub1.split(' ').nth(1).unwrap()).unwrap();
    let fp = format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(&blob)));
    assert_eq!(v, json!({"keys": [{"fingerprint": fp}], "namespace": "init-release"}));
    let o = h.run("", &["release", "keys", "--json"]);
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), r#"{"keys":[],"namespace":"init-release"}"#, "get.sh looks for this");
}

/// OpenSSH's own signature, made at run time: `ssh-keygen -Y sign` with a key it generates in the test home.
#[test]
fn what_ssh_keygen_signs_verifies() {
    let Some(kg) = ["/usr/bin/ssh-keygen", "/bin/ssh-keygen"].into_iter().find(|p| Path::new(p).is_file()) else {
        eprintln!("SKIPPED: no ssh-keygen (src/release.rs a_signature_made_by_ssh_keygen_verifies covers its format with a fixture)");
        return;
    };
    let h = home("openssh");
    let k = h.0.join("k");
    let ok = |c: &mut Command| assert!(c.stdin(Stdio::null()).status().unwrap().success());
    ok(Command::new(kg).args(["-q", "-t", "ed25519", "-N", "", "-C", "INIT TEST KEY", "-f"]).arg(&k));
    let data = b"a stand-in bundle\n";
    fs::write(h.0.join(BUNDLE), data).unwrap();
    let sums = format!("# INIT release SHA256SUMS v1\n# version: 0.5.4-preview\n# expires: {}\n{}  {BUNDLE}\n", date(30), hex(&Sha256::digest(data)));
    fs::write(h.0.join("SHA256SUMS"), sums).unwrap();
    ok(Command::new(kg).args(["-q", "-Y", "sign", "-n", "init-release", "-f"]).arg(&k).arg(h.0.join("SHA256SUMS")));
    let pub1 = fs::read_to_string(h.0.join("k.pub")).unwrap();
    let (code, v, err) = h.verify(pub1.trim(), &[]);
    assert_eq!((code, v["state"].as_str()), (Some(0), Some("unknown")), "{err}");
    let fp = String::from_utf8(Command::new(kg).arg("-lf").arg(h.0.join("k.pub")).output().unwrap().stdout).unwrap();
    assert_eq!(fp.split(' ').nth(1), v["key"].as_str(), "the fingerprint ssh-keygen -l prints");
}
