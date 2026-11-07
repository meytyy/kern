//! Release tooling for signed updates. The key file holds the 32-byte Ed25519
//! secret as hex; keep it outside the repository.
//!
//!   cargo run --example kern_update_key -- keygen <key-file>
//!   cargo run --example kern_update_key -- pubkey <key-file>
//!   cargo run --example kern_update_key -- sign <key-file> <version> <package>...
//!   cargo run --example kern_update_key -- verify <package>   (with <package>.sig)
use kern::core::update_sign::{
    PUBLIC_KEY_HEX, check_package, hex, public_key_of, sign, unhex, verify,
};
use std::path::Path;

fn read_key(path: &str) -> [u8; 32] {
    let text = std::fs::read_to_string(path).expect("read key file");
    unhex::<32>(&text).expect("key file: 64 hex characters")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") => {
            let path = Path::new(&args[1]);
            assert!(
                !path.exists(),
                "{} exists: not overwriting a key",
                path.display()
            );
            let mut secret = [0u8; 32];
            getrandom::fill(&mut secret).expect("random bytes");
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).unwrap();
            }
            std::fs::write(path, hex(&secret)).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
            println!("{}", public_key_of(&secret));
        }
        Some("pubkey") => println!("{}", public_key_of(&read_key(&args[1]))),
        Some("sign") => {
            let secret = read_key(&args[1]);
            let version = &args[2];
            for pkg in &args[3..] {
                let text = sign(&secret, version, Path::new(pkg)).expect("sign");
                let out = format!("{pkg}.sig");
                std::fs::write(&out, text).unwrap();
                println!("{out}");
            }
        }
        Some("verify") => {
            // What KERN does before installing: the embedded release key, then the bytes.
            let pkg = Path::new(&args[1]);
            let text = std::fs::read_to_string(format!("{}.sig", args[1])).expect("read .sig");
            let m = verify(&text, PUBLIC_KEY_HEX).expect("signature");
            check_package(pkg, &m, None).expect("package");
            println!(
                "ok: {} {} {} bytes sha256 {}",
                m.file, m.version, m.size, m.sha256
            );
        }
        _ => eprintln!(
            "usage: keygen <key> | pubkey <key> | sign <key> <version> <package>... | verify <package>"
        ),
    }
}
