extern crate rsign;
extern crate sodiumoxide;
extern crate base64;
extern crate libsodium_sys as ffi;
extern crate rpassword;
extern crate chrono;

use rsign::*;
use rsign::perror::PError;
use rsign::perror::Result;
use rsign::parse_args::parse_args;

use sodiumoxide::crypto::sign::{self, PublicKey, SecretKey, Signature, SIGNATUREBYTES};
use sodiumoxide::crypto::pwhash::{self, MemLimit, OpsLimit};
use chrono::prelude::*;

use std::fmt::Display;
use std::io::prelude::*;
use std::io::{BufWriter, BufReader};
use std::fs::{OpenOptions, File};
use std::path::Path;

use std::str::FromStr;


macro_rules! fatal {
    ($($tt:tt)*) => {{
        use std::io::Write;
        writeln!(&mut ::std::io::stderr(), $($tt)*).unwrap();
        ::std::process::exit(1)
    }}
}

fn create_file_rw<P: AsRef<Path>>(path: P) -> Result<BufWriter<File>> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| PError::from(e))
        .and_then(|file| {
                      Ok(BufWriter::new(file))
                  })
}

fn panic_if_file_exist<P>(path_pk: P, path_sk: P)
    where P: AsRef<Path> + Display
{
    if path_pk.as_ref().exists() || path_sk.as_ref().exists() {
        fatal!("Found keys at {} and {}\ntry using -f if you want to overwrite",
               path_pk,
               path_sk);
    }
}

fn get_password(prompt: &str) -> Result<String> {
    let pwd = rpassword::prompt_password_stdout(prompt)?;
    if pwd.len() == 0 {
        Err(PError::PasswordError("can't be blank".to_owned()))
    } else if pwd.len() > PASSWORDMAXBYTES {
        Err(PError::PasswordError("can't exceed 1024 bytes lenght".to_owned()))
    } else {
        Ok(pwd)
    }
}

fn generate_keys<P>(path_pk: P, path_sk: P, comment: Option<&str>, force: bool) -> Result<()>
    where P: AsRef<Path> + Copy + Display
{
    if !force {
        panic_if_file_exist(path_pk, path_sk);
    }
    let (pk_str, mut sk_str) = gen_keystruct();
    sk_str.checksum();
    let pwd = get_password("Password: ")?;
    let pwd2 = get_password("Password (one more time): ")?;
    if pwd != pwd2 {
        return Err(PError::PasswordError("passwords don't match!".to_owned()));
    }
    print!("Deriving a key from password... ");
    let salt = pwhash::Salt::from_slice(sk_str.kdf_salt.as_ref()).unwrap();
    let mut stream = vec![0u8; sk_str.keynum_sk.len()];
    pwhash::derive_key(stream.as_mut_slice(),
                       pwd.as_bytes(),
                       &salt,
                       OpsLimit(load_usize_le(&sk_str.kdf_opslimit_le)),
                       MemLimit(load_usize_le(&sk_str.kdf_memlimit_le)))
            .unwrap();
    println!("Done!");
    sk_str.xor_keynum(stream);

    let mut pk_buf = create_file_rw(path_pk)?;
    write!(pk_buf, "{}rsign public key: ", rsign::COMMENT_PREFIX)?;
    writeln!(pk_buf,
             "{:X}",
             rsign::load_usize_le(&pk_str.keynum_pk.keynum[..]))?;
    writeln!(pk_buf, "{}", base64::encode(&pk_str.bytes()))?;
    pk_buf.flush()?;

    let mut sk_buf = create_file_rw(path_sk)?;
    write!(sk_buf, "{}", rsign::COMMENT_PREFIX)?;
    if let Some(comment) = comment {
        writeln!(sk_buf, "{}", comment)?;
    } else {
        writeln!(sk_buf, "{}", rsign::SECRETKEY_DEFAULT_COMMENT)?;
    }
    writeln!(sk_buf, "{}", base64::encode(&sk_str.bytes()))?;
    sk_buf.flush()?;

    println!("\nThe secret key was saved as {} - Keep it secret!",
             path_sk);
    println!("The public key was saved as {} - That one can be public.\n",
             path_pk);
    println!("Files signed using this key pair can be verified with the following command:\n");
    println!("rsign verify -m <file> -P {}",
             base64::encode(pk_str.bytes().as_slice()));
    sodiumoxide::utils::memzero(&mut sk_str.keynum_sk.sk);
    sodiumoxide::utils::memzero(&mut sk_str.kdf_salt);
    sodiumoxide::utils::memzero(&mut sk_str.keynum_sk.chk);

    Ok(())
}

fn sk_load<P: AsRef<Path>>(sk_path: P) -> Result<SeckeyStruct> {
    let mut sk = OpenOptions::new()
        .read(true)
        .open(sk_path)
        .map_err(|e| PError::from(e))
        .and_then(|file| {
            let mut sk_buf = BufReader::new(file);
            let mut _comment = String::new();
            sk_buf.read_line(&mut _comment)?;
            let mut encoded_buf = String::new();
            sk_buf.read_line(&mut encoded_buf)?;
            base64::decode(encoded_buf.trim())
                .map_err(|e| PError::from(e))
                .and_then(|decoded_buf| SeckeyStruct::from(&decoded_buf[..]))

        })?;

    let pwd = get_password("Password: ")?;
    print!("Deriving a key from password... ");

    let salt = pwhash::Salt::from_slice(&sk.kdf_salt).unwrap();
    let mut stream = vec![0u8; sk.keynum_sk.len()];
    pwhash::derive_key(stream.as_mut_slice(),
                       pwd.as_bytes(),
                       &salt,
                       OpsLimit(load_usize_le(&sk.kdf_opslimit_le)),
                       MemLimit(load_usize_le(&sk.kdf_memlimit_le)))
            .unwrap();
    println!("Done!");
    sk.xor_keynum(stream);
    Ok(sk)
}

fn pk_load<P: AsRef<Path>>(pk_path: P) -> Result<PubkeyStruct> {
    let pk = OpenOptions::new()
        .read(true)
        .open(pk_path)
        .map_err(|e| PError::from(e))
        .and_then(|file| {
            let mut pk_buf = BufReader::new(file);
            let mut _comment = String::new();
            pk_buf.read_line(&mut _comment)?;
            let mut encoded_buf = String::new();
            pk_buf.read_line(&mut encoded_buf)?;
            base64::decode(encoded_buf.trim())
                    .map_err(|e| PError::from(e))
                    .and_then(|decoded_buf| {
                                PubkeyStruct::from(&decoded_buf)
                    })
        })?;
    Ok(pk)
}
fn pk_load_string(pk_string: &str) -> Result<PubkeyStruct> {
    let pk = String::from_str(pk_string)
    .map_err(|e| PError::from(e))
    .and_then(|string| {
        base64::decode(string.as_bytes())
            .map_err(|e| PError::from(e))
            .and_then(|decoded_string| {
                PubkeyStruct::from(&decoded_string)
        })
    })?;
    Ok(pk)
}

fn sign<P>(sk_key: SeckeyStruct,
           pk_key: Option<PubkeyStruct>,
           sig_file: Option<P>,
           message_file: P,
           trusted_comment: Option<&str>,
           untrusted_comment: Option<&str>,
           hashed: bool)
           -> Result<()>
    where P: AsRef<Path> + Copy + std::fmt::Display
{
    let t_comment = if let Some(trusted_comment) = trusted_comment {
        format!("{}", trusted_comment)
    } else {
        format!("timestamp:{} file:{}", Utc::now().timestamp(), message_file)
    };

    let unt_comment = if let Some(untrusted_comment) = untrusted_comment {
        format!("{}{}", COMMENT_PREFIX, untrusted_comment)
    } else {
        format!("{}{}", COMMENT_PREFIX, DEFAULT_COMMENT)
    };
    let msg_buf = load_message_file(message_file, &hashed)?;
    let sig_file_name = if let Some(file) = sig_file {
        format!("{}", file)
    } else {
        format!("{}{}", message_file, SIG_SUFFIX)
    };

    let mut sig_buf = create_file_rw(sig_file_name)?;
    let mut sig_str = SigStruct::default();
    if !hashed {
        sig_str.sig_alg = sk_key.sig_alg.clone();
    } else {
        sig_str.sig_alg = SIGALG_HASHED;
    }
    sig_str
        .keynum
        .copy_from_slice(&sk_key.keynum_sk.keynum[..]);

    let signature =
        sodiumoxide::crypto::sign::sign_detached(msg_buf.as_ref(),
                                                 &SecretKey::from_slice(sk_key
                                                                            .keynum_sk
                                                                            .sk
                                                                            .as_ref())
                                                          .unwrap());
    sig_str.sig.copy_from_slice(&signature[..]);

    let mut sig_and_trust_comment: Vec<u8> = vec![];
    sig_and_trust_comment.extend(sig_str.sig.iter());
    sig_and_trust_comment.extend(t_comment.as_bytes().iter());

    let global_sig =
        sodiumoxide::crypto::sign::sign_detached(sig_and_trust_comment.as_ref(),
                                                 &SecretKey::from_slice(&sk_key.keynum_sk.sk)
                                                      .unwrap());
    if let Some(pk_str) = pk_key {
        let pk = PublicKey::from_slice(&pk_str.keynum_pk.pk[..]).unwrap();
        if !sodiumoxide::crypto::sign::verify_detached(&global_sig, &sig_and_trust_comment, &pk) {
            panic!("Could not verify signature with the provided public key");
        } else {
            println!("Signature checked with the public key!");;
        }
    }

    writeln!(sig_buf, "{}", unt_comment)?;
    writeln!(sig_buf, "{}", base64::encode(&sig_str.bytes()))?;
    writeln!(sig_buf, "{}{}", TRUSTED_COMMENT_PREFIX, t_comment)?;
    writeln!(sig_buf, "{}", base64::encode(&global_sig[..]))?;
    sig_buf.flush()?;
    Ok(())
}

fn verify<P>(pk_key: PubkeyStruct, sig_file: P, message_file: P) -> Result<()>
    where P: AsRef<Path> + Copy + Display
{
    let mut hashed: bool = false;

    let mut trusted_comment: Vec<u8> = Vec::with_capacity(TRUSTEDCOMMENTMAXBYTES);
    let mut global_sig: Vec<u8> = Vec::with_capacity(SIGNATUREBYTES);
    let sig = sig_load(sig_file, &mut global_sig, &mut trusted_comment, &mut hashed);

    let message = load_message_file(message_file, &hashed)?;
    if sig.keynum != pk_key.keynum_pk.keynum {
        fatal!("Public key ID: {:X} is not equal to signature key ID: {:X}",
               rsign::load_usize_le(&pk_key.keynum_pk.keynum[..]),
               rsign::load_usize_le(&sig.keynum[..]));
    }
    if sign::verify_detached(&Signature::from_slice(&sig.sig[..]).unwrap(),
                             &message[..],
                             &PublicKey::from_slice(&pk_key.keynum_pk.pk[..]).unwrap()) {
        println!("Signature verified!");
    }
    Ok(())
}
fn sig_load<P>(sig_file: P,
               global_sig: &mut Vec<u8>,
               trusted_comment: &mut Vec<u8>,
               hashed: &mut bool)
               -> SigStruct
    where P: AsRef<Path> + Copy
{
    let mut buf_r = BufReader::new(File::open(sig_file).unwrap());
    let mut untrusted_comment = String::with_capacity(COMMENTBYTES);
    let _ = buf_r.read_line(&mut untrusted_comment);
    if !untrusted_comment.starts_with(COMMENT_PREFIX) {
        panic!("untrusted comment should start with: {}", COMMENT_PREFIX);
    }
    let mut sig_str = String::with_capacity(74);
    let _ = buf_r.read_line(&mut sig_str);
    let sig_str = base64::decode(sig_str.trim().as_bytes()).unwrap();
    let mut t_comment = String::with_capacity(TRUSTEDCOMMENTMAXBYTES);
    let _ = buf_r.read_line(&mut t_comment);
    if !t_comment.starts_with(TRUSTED_COMMENT_PREFIX) {
        panic!("trusted comment should start with: {}",
               TRUSTED_COMMENT_PREFIX);
    }
    let _ = t_comment.drain(..TR_COMMENT_PREFIX_LEN).count();

    trusted_comment.extend_from_slice(t_comment.trim().as_bytes());
    let mut g_sig = String::with_capacity(SIGNATUREBYTES);
    let _ = buf_r.read_line(&mut g_sig);
    global_sig.extend_from_slice(g_sig.trim().as_bytes());
    let sig = SigStruct::from(&sig_str[..]).unwrap();

    if sig.sig_alg == SIGALG {
        *hashed = false;
    } else if sig.sig_alg == SIGALG_HASHED {
        *hashed = true;
    } else {
        fatal!("Unsupported signature algorithm");
    }
    sig
}

fn load_message_file<P>(message_file: P, hashed: &bool) -> Result<Vec<u8>>
    where P: AsRef<Path> + Copy + Display
{
    if *hashed {
        return hash_message_file(message_file);
    }
    OpenOptions::new()
        .read(true)
        .open(message_file)
        .map_err(|e| PError::from(e))
        .and_then(|mut file| {
                      if file.metadata().unwrap().len() > (1u64 << 30) {
                          fatal!("File {} is larger than 1G try using -H", message_file);
                      }
                      let mut msg_buf: Vec<u8> = Vec::new();
                      file.read_to_end(&mut msg_buf)?;
                      Ok(msg_buf)
                  })
}

fn hash_message_file<P>(message_file: P) -> Result<Vec<u8>>
    where P: AsRef<Path> + Copy + Display
{
    OpenOptions::new()
        .read(true)
        .open(message_file)
        .map_err(|e| PError::from(e))
        .and_then(|file| {
            let mut buf_reader = BufReader::new(file);
            let mut buf_chunk = [0u8; 65536];
            let state_sz = unsafe { ffi::crypto_generichash_statebytes() };
            let mut state: Vec<u8> = vec![0;state_sz];
            let ptr_state = state.as_mut_ptr() as *mut ffi::crypto_generichash_state;
            generichash::init(ptr_state).unwrap();
            while buf_reader.read(&mut buf_chunk).unwrap() > 0 {
                generichash::update(ptr_state, &buf_chunk).unwrap();
            }
            Ok(generichash::finalize(ptr_state)
                   .unwrap()
                   .as_ref()
                   .to_vec())
        })

}


fn main() {

    let args = parse_args();
    sodiumoxide::init();

    if let Some(generate_action) = args.subcommand_matches("generate") {
        //TODO: add parent folder to sk_file_path
        let _ = generate_keys(generate_action
                                  .value_of("pk_path")
                                  .expect("pk file path"),
                              generate_action
                                  .value_of("sk_path")
                                  .expect("sk file path"),
                              generate_action.value_of("comment"),
                              generate_action.is_present("force"))
                .unwrap_or_else(|e| e.exit());

    }

    if let Some(sign_action) = args.subcommand_matches("sign") {
        let sk_file = sign_action.value_of("sk_path").unwrap();
        let mut pk: Option<PubkeyStruct> = None;
        if sign_action.is_present("pk_path") {
            if let Some(filename) = sign_action.value_of("pk_path") {
                pk = Some(pk_load(filename).unwrap());
            }
        } else if sign_action.is_present("public_key") {
            if let Some(string) = sign_action.value_of("public_key") {
                pk = Some(pk_load_string(string).unwrap());
            }
        }

        let sk = sk_load(sk_file).unwrap_or_else(|e| e.exit());
        let _ = sign(sk,
                     pk,
                     sign_action.value_of("sig_file"),
                     sign_action.value_of("message").unwrap(),
                     sign_action.value_of("trusted-comment"),
                     sign_action.value_of("untrusted-comment"),
                     sign_action.is_present("hash"));
    }

    if let Some(verify_action) = args.subcommand_matches("verify") {
        let pk = pk_load(verify_action.value_of("pk_path").unwrap()).unwrap();
        let sig_file = verify_action.value_of("sig_file").unwrap();
        let message_file = verify_action.value_of("file").unwrap();
        let _ = verify(pk, sig_file, message_file);
    }


}