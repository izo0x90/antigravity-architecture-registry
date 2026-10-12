//! Each kind of install step, against local fixtures: a built .deb (xz and
//! zstd data), zip and tar.gz with only some members taken, links, a system
//! group (with a fake group file and tool), a certificate from a fake store,
//! an archive behind a launcher patched, and the time limits.

use std::io::Write;
use std::path::{Path, PathBuf};

use nah_packs::{Catalog, Distribution, Lock, Places};

fn shipped(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs").join(name)).unwrap()
}

fn sha256(path: &Path) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(std::fs::read(path).unwrap()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A tar of `files` (path, text, mode).
fn tar_of(files: &[(&str, &str, u32)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, text, mode) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(*mode);
        header.set_cksum();
        builder.append_data(&mut header, path, text.as_bytes()).unwrap();
    }
    builder.into_inner().unwrap()
}

/// A Debian package whose data is `files`, compressed as `compression`.
fn deb(path: &Path, files: &[(&str, &str, u32)], compression: &str) {
    let data = tar_of(files);
    let packed = match compression {
        "xz" => {
            let mut out = Vec::new();
            lzma_rs::xz_compress(&mut std::io::BufReader::new(&data[..]), &mut out).unwrap();
            out
        }
        "zst" => ruzstd::encoding::compress_to_vec(&data[..], ruzstd::encoding::CompressionLevel::Fastest),
        other => panic!("{other}"),
    };
    let mut ar = ar::Builder::new(std::fs::File::create(path).unwrap());
    let mut add = |name: &str, bytes: &[u8]| ar.append(&ar::Header::new(name.as_bytes().to_vec(), bytes.len() as u64), bytes).unwrap();
    add("debian-binary", b"2.0\n");
    add(&format!("control.tar.{compression}"), &packed);
    add(&format!("data.tar.{compression}"), &packed);
}

fn zip_of(path: &Path, files: &[(&str, &str)], prefix: &[u8]) {
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(prefix).unwrap();
    let mut w = zip::ZipWriter::new(file);
    for (name, text) in files {
        w.start_file(*name, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).unix_permissions(0o755)).unwrap();
        w.write_all(text.as_bytes()).unwrap();
    }
    w.finish().unwrap();
}

fn tar_gz(path: &Path, files: &[(&str, &str, u32)]) {
    let mut gz = flate2::write::GzEncoder::new(std::fs::File::create(path).unwrap(), flate2::Compression::fast());
    gz.write_all(&tar_of(files)).unwrap();
    gz.finish().unwrap();
}

fn certificate(subject: &str) -> String {
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.distinguished_name.push(rcgen::DnType::CommonName, subject);
    params.self_signed(&rcgen::KeyPair::generate().unwrap()).unwrap().pem()
}

struct World {
    dir: tempfile::TempDir,
}

impl World {
    fn at(&self, p: &str) -> PathBuf {
        self.dir.path().join(p)
    }

    /// The shipped index and platforms (this machine's platform keeping its
    /// groups in `groups`, created by `add_group`), and `pack`.
    fn catalog(&self, pack: &str) -> Catalog {
        let mut platforms = shipped("platforms.toml");
        let groups = format!("groups = {{ file = \"{}\", create = [\"{}\"] }}", self.at("group").display(), self.at("add_group").display());
        platforms = platforms.replace("groups = { file = \"/etc/group\", create = [\"groupadd\", \"-r\"] }", &groups);
        let files = vec![
            ("index.toml".to_owned(), shipped("index.toml").into_bytes()),
            ("platforms.toml".to_owned(), platforms.into_bytes()),
            ("harness/t/pack.toml".to_owned(), pack.as_bytes().to_vec()),
        ];
        let at = |p: &str| self.at(p);
        Catalog::load(&Distribution::new(files), &Places { project: at("project"), home: at("home"), config_home: at("config"), data_home: at("data") }).unwrap()
    }
}

fn nah_packs_root() -> bool {
    #[cfg(unix)]
    return rustix::process::geteuid().is_root();
    #[cfg(not(unix))]
    return false;
}

fn world() -> World {
    let w = World { dir: tempfile::tempdir().unwrap() };
    // Not root, the group can't be created: it's there already then.
    let groups = if nah_packs_root() { "root:x:0:\n" } else { "root:x:0:\nnobody:x:65534:\n" };
    std::fs::write(w.at("group"), groups).unwrap();
    // A group tool: appends the name it's given to the group file.
    std::fs::write(w.at("add_group"), format!("#!/bin/sh\necho \"$1:x:999:\" >> {}\n", w.at("group").display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(w.at("add_group"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    w
}

/// A harness pack for every platform with `steps`, providing `launch`.
fn pack(steps: &str) -> String {
    format!(
        "[pack]\nname = \"t\"\nkind = \"harness\"\nversion = \"1.0.0\"\ndescription = \"Steps.\"\n[tool]\nname = \"t\"\nversion = \"1.0.0\"\nsupports = \">=1.0.0\"\n[[install]]\nplatforms = [\"linux-x86_64\", \"linux-arm64\", \"linux-arm64-16k\", \"macos-arm64\", \"macos-x86_64\"]\n{steps}\n[provides]\ndriver = \"t\"\nlaunch = {{ program = \"{{dest}}/bin/server\" }}\n"
    )
}

fn download(url: &Path, unpack: &str, extra: &str) -> String {
    format!("[[install.steps]]\ndownload = {{ url = \"file://{}\", sha256 = \"{}\", unpack = \"{unpack}\"{extra} }}\n", url.display(), sha256(url))
}

#[test]
fn every_kind_of_step_installs_from_local_fixtures() {
    let w = world();
    // The server, from a zip holding more than is wanted.
    zip_of(&w.at("server.zip"), &[("server", "#!/bin/sh\necho server\n"), ("README", "not wanted")], b"");
    // An emulator from a Debian package (xz), a library from another (zstd).
    deb(&w.at("emu.deb"), &[("./usr/bin/emu", "emu", 0o755), ("./usr/share/doc/emu/copyright", "not wanted", 0o644)], "xz");
    deb(&w.at("lib.deb"), &[("./lib/x86_64-linux-gnu/libc.so.6", "libc", 0o644), ("./lib/x86_64-linux-gnu/ld.so", "ld", 0o755)], "zst");
    tar_gz(&w.at("extra.tar.gz"), &[("extra/tool", "tool", 0o755), ("extra/docs.txt", "not wanted", 0o644)]);
    // The system store, with the proxy's certificate among others.
    let proxy = certificate("Test Sandboxes Proxy CA");
    std::fs::write(w.at("store.pem"), format!("{}{proxy}{}", certificate("Some Root"), certificate("Other Root"))).unwrap();
    // An archive behind a launcher, with a CA bundle in it.
    zip_of(&w.at("app.par"), &[("certs/cacert.pem", "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n"), ("main.py", "print(1)\n")], b"#!/bin/sh\nexec python3 \"$0\" \"$@\"\n");
    // The same, with the launcher put in front of a finished zip (its
    // offsets count from the zip's start, as the real one's do).
    zip_of(&w.at("plain.zip"), &[("certs/cacert.pem", "-----BEGIN CERTIFICATE-----\nBBBB\n-----END CERTIFICATE-----\n"), ("main.py", "print(2)\n")], b"");
    let mut catted = b"\x7fELF launcher bytes\n".to_vec();
    catted.extend(std::fs::read(w.at("plain.zip")).unwrap());
    std::fs::write(w.at("cat.par"), catted).unwrap();
    // One whose archive can't change size (a section of a program): a bundle
    // of three certificates, with room for the proxy's only if one goes.
    let roots = [certificate("Root One"), certificate("Root Two"), certificate("Root Three")];
    zip_of(&w.at("fixed.zip"), &[("certs/roots.pem", &roots.concat()), ("main.py", "print(3)\n")], b"");
    let mut fixed = b"\x7fELF fixed section\n".to_vec();
    fixed.extend(std::fs::read(w.at("fixed.zip")).unwrap());
    std::fs::write(w.at("fixed.par"), fixed).unwrap();

    let steps = [
        download(&w.at("server.zip"), "zip", ", into = \"bin\", extract = [\"server\"]"),
        download(&w.at("emu.deb"), "deb", ", into = \"emu\", extract = [\"usr/bin/emu\"]"),
        download(&w.at("lib.deb"), "deb", ", into = \"sysroot\""),
        download(&w.at("extra.tar.gz"), "tar_gz", ", extract = [\"extra/tool\"]"),
        format!("[[install.steps]]\ndownload = {{ url = \"file://{}\", sha256 = \"{}\", save_as = \"app.par\", into = \"bin\" }}\n", w.at("app.par").display(), sha256(&w.at("app.par"))),
        "[[install.steps]]\nlink = { path = \"sysroot/lib64/ld.so\", target = \"../lib/x86_64-linux-gnu/ld.so\" }\n".to_owned(),
        "[[install.steps]]\nensure_group = { name = \"nobody\" }\n".to_owned(),
        format!("[[install.steps]]\nsystem_ca = {{ store = \"{}\", subject = \"Sandboxes Proxy CA\", save_as = \"proxy-ca.pem\" }}\n", w.at("store.pem").display()),
        "[[install.steps]]\npatch_archive = { file = \"bin/app.par\", members = [\"certs/*.pem\"], append = \"proxy-ca.pem\" }\n".to_owned(),
        format!("[[install.steps]]\ndownload = {{ url = \"file://{}\", sha256 = \"{}\", save_as = \"cat.par\", into = \"bin\" }}\n", w.at("cat.par").display(), sha256(&w.at("cat.par"))),
        "[[install.steps]]\npatch_archive = { file = \"bin/cat.par\", members = [\"certs/*.pem\"], append = \"proxy-ca.pem\" }\n".to_owned(),
        format!("[[install.steps]]\ndownload = {{ url = \"file://{}\", sha256 = \"{}\", save_as = \"fixed.par\", into = \"bin\" }}\n", w.at("fixed.par").display(), sha256(&w.at("fixed.par"))),
        "[[install.steps]]\npatch_archive = { file = \"bin/fixed.par\", members = [\"certs/*.pem\"], append = \"proxy-ca.pem\", keep_size = { block = \"-----BEGIN CERTIFICATE-----\", pad = \"#\" } }\n".to_owned(),
    ];
    let catalog = w.catalog(&pack(&steps.concat()));
    let platform = catalog.platforms.current().unwrap().clone();
    let installed = nah_packs::install(&catalog, "t", &platform).unwrap();
    let dest = &installed.dest;
    let read = |p: &str| std::fs::read_to_string(dest.join(p)).unwrap();

    // Only the members asked for.
    assert_eq!(read("bin/server"), "#!/bin/sh\necho server\n");
    assert!(!dest.join("bin/README").exists() && !dest.join("emu/usr/share").exists() && !dest.join("extra/docs.txt").exists());
    assert_eq!((read("emu/usr/bin/emu"), read("sysroot/lib/x86_64-linux-gnu/libc.so.6"), read("extra/tool")), ("emu".into(), "libc".into(), "tool".into()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert!(std::fs::metadata(dest.join("emu/usr/bin/emu")).unwrap().permissions().mode() & 0o111 != 0, "modes are kept");
    }
    // A relative link inside the install folder.
    assert_eq!(std::fs::read_link(dest.join("sysroot/lib64/ld.so")).unwrap(), Path::new("../lib/x86_64-linux-gnu/ld.so"));
    assert_eq!(read("sysroot/lib64/ld.so"), "ld");
    // The missing group, created by the platform's tool (as root; the
    // step's own test covers not being root).
    if nah_packs_root() {
        assert!(std::fs::read_to_string(w.at("group")).unwrap().contains("nobody:x:999:"));
    }
    // The certificate by its subject, and the archive's bundle patched, its
    // launcher kept, the rest untouched.
    assert_eq!(read("proxy-ca.pem"), proxy);
    let par = std::fs::read(dest.join("bin/app.par")).unwrap();
    assert!(par.starts_with(b"#!/bin/sh\nexec python3"), "the launcher is kept");
    let mut archive = zip::ZipArchive::new(std::fs::File::open(dest.join("bin/app.par")).unwrap()).unwrap();
    let mut text = |name: &str| std::io::read_to_string(archive.by_name(name).unwrap()).unwrap();
    assert!(text("certs/cacert.pem").starts_with("-----BEGIN CERTIFICATE-----\nAAAA") && text("certs/cacert.pem").ends_with(&proxy));
    assert_eq!(text("main.py"), "print(1)\n");
    // Python reads it as the launcher will.
    let python = std::process::Command::new("python3").arg("-c").arg(format!("import zipfile; z = zipfile.ZipFile('{}'); assert z.testzip() is None; print(z.read('main.py').decode())", dest.join("bin/app.par").display())).output();
    if let Ok(out) = python {
        assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("print(1)"), "{}", String::from_utf8_lossy(&out.stderr));
    }
    // Offsets counted from the zip's start: the launcher kept, the members read back.
    let cat = dest.join("bin/cat.par");
    assert!(std::fs::read(&cat).unwrap().starts_with(b"\x7fELF launcher bytes\n"));
    let script = "import sys, zipfile\nz = zipfile.ZipFile(sys.argv[1])\nassert z.testzip() is None\nt = z.read('certs/cacert.pem').decode()\nassert t.startswith('-----BEGIN CERTIFICATE-----\\nBBBB') and t.endswith(sys.argv[2]), t\nassert z.read('main.py') == b'print(2)\\n'\n";
    if let Ok(out) = std::process::Command::new("python3").arg("-c").arg(script).arg(&cat).arg(&proxy).output() {
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    // Kept to its size: the same bytes long, the CA first, whole
    // certificates after it, the rest a comment line; the CRCs right.
    let fixed = dest.join("bin/fixed.par");
    assert_eq!(std::fs::metadata(&fixed).unwrap().len(), std::fs::metadata(w.at("fixed.par")).unwrap().len(), "the same size");
    let script = "import sys, zipfile\nz = zipfile.ZipFile(sys.argv[1])\nassert z.testzip() is None\nt = z.read('certs/roots.pem').decode()\nassert t.startswith(sys.argv[2]), t[:80]\nbody = t[len(sys.argv[2]):]\nassert body.count('-----BEGIN CERTIFICATE-----') == body.count('-----END CERTIFICATE-----') >= 1, body\nassert body.rstrip('\\n').endswith('#'), body[-40:]\nassert z.read('main.py') == b'print(3)\\n'\n";
    if let Ok(out) = std::process::Command::new("python3").arg("-c").arg(script).arg(&fixed).arg(&proxy).output() {
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    assert_eq!(Lock::read(&catalog.lock_path).unwrap().packs["t"], installed);
}

#[test]
fn optional_steps_skip_and_others_fail_clearly() {
    let w = world();
    std::fs::write(w.at("store.pem"), certificate("Some Root")).unwrap();
    zip_of(&w.at("server.zip"), &[("server", "s")], b"");
    let base = download(&w.at("server.zip"), "zip", ", into = \"bin\"");
    let ca = |optional: bool| format!("[[install.steps]]\nsystem_ca = {{ store = \"{}\", subject = \"Proxy CA\", save_as = \"ca.pem\", optional = {optional} }}\n[[install.steps]]\npatch_archive = {{ file = \"bin/server\", members = [\"*\"], append = \"ca.pem\", optional = {optional} }}\n", w.at("store.pem").display());
    let catalog = w.catalog(&pack(&format!("{base}{}", ca(true))));
    let platform = catalog.platforms.current().unwrap().clone();
    nah_packs::install(&catalog, "t", &platform).expect("no proxy CA: both steps skip");
    let catalog = w.catalog(&pack(&format!("{base}{}", ca(false))));
    let err = nah_packs::install(&catalog, "t", &platform).unwrap_err().to_string();
    assert!(err.contains("no certificate whose subject has 'Proxy CA'"), "{err}");
    // A member that isn't in the archive.
    let catalog = w.catalog(&pack(&download(&w.at("server.zip"), "zip", ", extract = [\"nope\"]")));
    assert!(nah_packs::install(&catalog, "t", &platform).unwrap_err().to_string().contains("has no 'nope'"));
    // Bad steps are refused when the pack loads.
    for (step, says) in [
        ("link = { path = \"../out\", target = \"x\" }", "inside the install folder"),
        ("link = { path = \"a\", target = \"/abs\" }", "must be relative"),
        ("ensure_group = { name = \"no group\" }", "isn't a group name"),
        ("patch_archive = { file = \"a\", members = [], append = \"b\" }", "members is empty"),
        ("link = { path = \"bin/a\", target = \"../../../etc/passwd\" }", "leads out of the install folder"),
        ("download = { url = \"file:///x\", sha256 = \"0000000000000000000000000000000000000000000000000000000000000000\", unpack = \"zip\", into = \"../../escaped\" }", "inside the install folder"),
        ("download = { url = \"file:///x\", sha256 = \"0000000000000000000000000000000000000000000000000000000000000000\", save_as = \"../out.bin\" }", "inside the install folder"),
    ] {
        let files = vec![
            ("index.toml".to_owned(), shipped("index.toml").into_bytes()),
            ("platforms.toml".to_owned(), shipped("platforms.toml").into_bytes()),
            ("harness/t/pack.toml".to_owned(), pack(&format!("{base}[[install.steps]]\n{step}\n")).into_bytes()),
        ];
        let err = Catalog::load(&Distribution::new(files), &Places::isolated(&w.at("x"))).unwrap_err().to_string();
        assert!(err.contains(says), "{step}: {err}");
    }
}

#[test]
fn a_step_that_runs_too_long_is_stopped() {
    let w = world();
    let catalog = w.catalog(&pack("[[install.steps]]\nrun = { command = \"sleep\", args = [\"30\"], timeout_s = 1 }\n"));
    let platform = catalog.platforms.current().unwrap().clone();
    let started = std::time::Instant::now();
    let err = nah_packs::install(&catalog, "t", &platform).unwrap_err().to_string();
    assert!(err.contains("took longer than 1 s"), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert!(catalog.timeouts.download.as_secs() > 0 && catalog.timeouts.run.as_secs() > 0, "the defaults come from index.toml");
}

/// A harness that answers ACP `initialize` (or, `silent`, reads and never answers).
fn acp_server(path: &Path, version: &str, silent: bool) -> String {
    let answer = format!("echo '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":1,\"agentInfo\":{{\"name\":\"fake\",\"version\":\"{version}\"}}}}}}'");
    let body = if silent { "sleep 60".to_owned() } else { answer };
    format!("#!/bin/sh\nread line\n{body}\n").replace("{path}", &path.display().to_string())
}

#[test]
fn the_acp_check_starts_the_harness_and_reads_its_version() {
    let w = world();
    let make = |name: &str, script: &str| {
        let zip_path = w.at(name);
        zip_of(&zip_path, &[("server", script)], b"");
        zip_path
    };
    let harness = |zip: &Path, supports: &str| {
        format!(
            "[pack]\nname = \"t\"\nkind = \"harness\"\nversion = \"1.0.0\"\ndescription = \"x\"\n[tool]\nname = \"t\"\nsupports = \"{supports}\"\n[tool.check]\nacp = {{ timeout_s = 2 }}\n[[install]]\nplatforms = [\"linux-x86_64\", \"linux-arm64\", \"linux-arm64-16k\", \"macos-arm64\", \"macos-x86_64\"]\n{}\n[provides]\ndriver = \"t\"\nlaunch = {{ program = \"{{dest}}/server\", args = [\"--uid={{user}}\"] }}\n",
            download(zip, "zip", "")
        )
    };
    let answering = make("answers.zip", &acp_server(Path::new("x"), "1.4.0", false));
    let catalog = w.catalog(&harness(&answering, ">=1.0.0, <2.0.0"));
    let platform = catalog.platforms.current().unwrap().clone();
    let installed = nah_packs::install(&catalog, "t", &platform).unwrap();
    assert_eq!(installed.tool_version.as_deref(), Some("1.4.0"), "the version it answered with is recorded");
    let lock = Lock::read(&catalog.lock_path).unwrap();
    assert_eq!(nah_packs::state(&catalog, &lock, "t").unwrap(), nah_packs::State::Ready { tool_version: Some("1.4.0".into()) });
    // Out of the pack's range.
    let narrow = w.catalog(&harness(&answering, ">=2.0.0"));
    assert!(matches!(nah_packs::state(&narrow, &lock, "t").unwrap(), nah_packs::State::OutOfRange { .. }));
    // A harness that never answers fails its check within the limit, and isn't installed.
    let silent = make("silent.zip", &acp_server(Path::new("x"), "1.4.0", true));
    let catalog = w.catalog(&harness(&silent, ">=1.0.0, <2.0.0"));
    let started = std::time::Instant::now();
    let err = nah_packs::install(&catalog, "t", &platform).unwrap_err().to_string();
    assert!(err.contains("fails its check") && err.contains("no answer to ACP initialize within 2 s"), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    assert_eq!(Lock::read(&catalog.lock_path).unwrap().packs["t"], installed, "the working install stays");
}
