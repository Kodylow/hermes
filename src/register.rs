use std::str::FromStr;

use crate::{
    models::app_user::{AppUser, NewAppUser},
    routes::{RegisterRequest, RegisterResponse},
    State,
};
use fedimint_core::api::InviteCode;
use lazy_regex::*;
use log::error;
use names::Generator;
use nostr::PublicKey;
use reqwest::StatusCode;

pub static ALPHANUMERIC_REGEX: Lazy<Regex> = lazy_regex!("^[a-z0-9-_.]+$");

pub fn is_valid_name(name: &str) -> bool {
    let name_len = name.len();
    if !(2..=30).contains(&name_len) {
        return false;
    }

    ALPHANUMERIC_REGEX.is_match(name)
}

pub fn check_available(state: &State, name: String) -> anyhow::Result<bool> {
    if !is_valid_name(&name) {
        return Ok(false);
    }

    state.db.check_name_available(name)
}

pub fn check_registered_pubkey(state: &State, pubkey: String) -> anyhow::Result<Option<String>> {
    state.db.check_registered_pubkey(pubkey)
}

pub fn get_user_by_pubkey(state: &State, pubkey: String) -> anyhow::Result<Option<AppUser>> {
    state.db.get_user_by_pubkey(pubkey)
}

pub fn change_user_federation(
    state: &State,
    user: AppUser,
    federation_id: String,
    federation_invite_code: String,
) -> anyhow::Result<()> {
    state
        .db
        .update_user_federation(user, federation_id, federation_invite_code)
}

pub fn disable_user_zaps(state: &State, user: AppUser) -> anyhow::Result<()> {
    state.db.disable_user_zaps(user)
}

pub fn generate_random_name(state: &State) -> anyhow::Result<String> {
    loop {
        let new_name = Generator::with_naming(names::Name::Numbered)
            .next()
            .expect("should generate name")
            .replace('-', "");

        if check_available(state, new_name.clone())? {
            return Ok(new_name);
        }
    }
}

pub async fn register(
    state: &State,
    req: RegisterRequest,
) -> Result<RegisterResponse, (StatusCode, String)> {
    // validate user name & pubkey first
    let requested_paid = req.name.is_some();
    if requested_paid && !is_valid_name(&req.name.clone().unwrap()) {
        return Err((StatusCode::BAD_REQUEST, "Unavailable".to_string()));
    }
    PublicKey::from_str(&req.pubkey)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Nostr Pubkey Invalid".to_string()))?;

    let name_to_register = if requested_paid {
        req.name.clone().unwrap().clone()
    } else {
        match generate_random_name(state) {
            Ok(s) => s,
            Err(e) => {
                error!("Error in register name generator: {e:?}");
                return Err((StatusCode::INTERNAL_SERVER_ERROR, "ServerError".to_string()));
            }
        }
    };

    match state.db.check_name_available(name_to_register.clone()) {
        Ok(true) => (),
        Ok(false) => {
            return Err((StatusCode::BAD_REQUEST, "Unavailable".to_string()));
        }
        Err(e) => {
            error!("Error in register: {e:?}");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "ServerError".to_string()));
        }
    };

    // make sure the federation is either already added or connectable
    let invite_code = match InviteCode::from_str(&req.federation_invite_code) {
        Ok(i) => i,
        Err(e) => {
            error!("Error in register: {e:?}");
            return Err((StatusCode::BAD_REQUEST, "InvalidFederation".to_string()));
        }
    };
    let federation_id = invite_code.federation_id();
    ensure_added_federation(state, federation_id, invite_code).await?;

    let new_user = NewAppUser {
        pubkey: req.pubkey,
        name: name_to_register.clone(),
        federation_id: federation_id.to_string(),
        federation_invite_code: req.federation_invite_code,
    };
    match state.db.insert_new_user(new_user) {
        Ok(_) => Ok(RegisterResponse {
            name: name_to_register,
        }),
        Err(e) => {
            error!("Errorgister: {e:?}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "ServerError".to_string()))
        }
    }
}

pub(crate) async fn ensure_added_federation(
    state: &State,
    federation_id: fedimint_core::config::FederationId,
    invite_code: InviteCode,
) -> Result<(), (StatusCode, String)> {
    if !state.mm.check_has_federation(federation_id).await {
        match state.mm.register_new_federation(invite_code).await {
            Ok(_) => (),
            Err(e) => {
                error!("Error in register: {e:?}");
                return Err((StatusCode::BAD_REQUEST, "InvalidFederation".to_string()));
            }
        }
    }
    Ok(())
}

#[cfg(all(test, not(feature = "integration-tests")))]
mod tests {
    use crate::register::is_valid_name;

    #[tokio::test]
    async fn check_name() {
        // bad names
        assert!(!is_valid_name("thisisoverthe30characternamelimit"));
        assert!(!is_valid_name("thisisoverthe30characternamelimit"));
        assert!(!is_valid_name("no!"));
        assert!(!is_valid_name("n"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("bad&name"));
        assert!(!is_valid_name("BADNAME"));
        assert!(!is_valid_name("bad space name"));

        // good
        assert!(is_valid_name("good_name"));
        assert!(is_valid_name("good.name"));
        assert!(is_valid_name("goodname"));
        assert!(is_valid_name("goodname1"));
        assert!(is_valid_name("yesnameisverygoodandunderlimit"));
    }
}

#[cfg(all(test, feature = "integration-tests"))]
use tbs::{AggregatePublicKey, BlindedMessage, BlindedSignature, SecretKeyShare};

#[cfg(all(test, feature = "integration-tests"))]
use sha2::Digest;

#[cfg(all(test, feature = "integration-tests"))]
#[derive(Debug, Copy, Clone)]
pub struct BlindSigner {
    sk: SecretKeyShare,
    pub pk: AggregatePublicKey,
}

#[cfg(all(test, feature = "integration-tests"))]
impl BlindSigner {
    pub fn derive(seed: &[u8], service_id: i32, plan_id: i32) -> Self {
        let hash = sha2::Sha512::digest(
            [seed, &service_id.to_be_bytes(), &plan_id.to_be_bytes()].concat(),
        );
        let scalar =
            tbs::Scalar::from_bytes_wide(hash.as_slice().try_into().expect("Sha512 is 64 bytes"));
        Self::from_sk(SecretKeyShare(scalar))
    }

    pub fn from_sk(sk: SecretKeyShare) -> Self {
        let pk = AggregatePublicKey(sk.to_pub_key_share().0);

        Self { sk, pk }
    }

    pub fn blind_sign(&self, blinded_message: BlindedMessage) -> BlindedSignature {
        let share = tbs::sign_blinded_msg(blinded_message, self.sk);
        BlindedSignature(share.0)
    }
}

#[cfg(all(test, feature = "integration-tests"))]
mod tests_integration {
    use std::{str::FromStr, sync::Arc};

    use fedimint_core::{api::InviteCode, config::FederationId, PeerId};
    use nostr::Keys;
    use secp256k1::Secp256k1;
    use tbs::{blind_message, unblind_signature, BlindingKey};

    use crate::{
        db::setup_db,
        mint::MockMultiMintWrapperTrait,
        models::app_user::NewAppUser,
        register::{check_available, register, BlindSigner},
        routes::RegisterRequest,
        State,
    };

    #[tokio::test]
    pub async fn test_username_checker() {
        dotenv::dotenv().ok();
        let pg_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let db = setup_db(pg_url);

        // swap out fm with a mock here since that's not what is being tested
        let mock_mm = Arc::new(MockMultiMintWrapperTrait::new());

        let nostr_nsec_str = std::env::var("NSEC").expect("FM_DB_PATH must be set");
        let nostr_sk = Keys::from_str(&nostr_nsec_str).expect("Invalid NOSTR_SK");
        let nostr = nostr_sdk::Client::new(&nostr_sk);

        // create blind signer
        let free_signer = BlindSigner::derive(&[0u8; 32], 0, 0);
        let paid_signer = BlindSigner::derive(&[0u8; 32], 0, 0);

        let state = State {
            db: db.clone(),
            mm: mock_mm,
            secp: Secp256k1::new(),
            nostr,
            free_pk: free_signer.pk,
            paid_pk: paid_signer.pk,
            domain: "http://127.0.0.1:8080".to_string(),
            nostr_sk,
        };

        let name = "veryuniquename123".to_string();
        let available = check_available(&state, name).expect("should get");
        assert!(available);

        let commonname = "commonname".to_string();
        let common_app_user = NewAppUser {
            pubkey: "".to_string(),
            name: commonname.clone(),
            federation_id: "".to_string(),
            unblinded_msg: "test_username_checker".to_string(),
            federation_invite_code: "".to_string(),
        };

        // don't care about error if already exists
        let _ = state.db.insert_new_user(common_app_user);

        let available = check_available(&state, commonname).expect("should get");
        assert!(!available);
    }

    #[tokio::test]
    pub async fn register_username_tests() {
        dotenv::dotenv().ok();
        let pg_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let db = setup_db(pg_url);

        // swap out fm with a mock here since that's not what is being tested
        let mut mock_mm = MockMultiMintWrapperTrait::new();
        mock_mm
            .expect_check_has_federation()
            .times(1)
            .returning(|_| true);

        // nostr
        let nostr_nsec_str = std::env::var("NSEC").expect("FM_DB_PATH must be set");
        let nostr_sk = Keys::from_str(&nostr_nsec_str).expect("Invalid NOSTR_SK");
        let nostr = nostr_sdk::Client::new(&nostr_sk);

        // create blind signer
        let free_signer = BlindSigner::derive(&[0u8; 32], 0, 0);
        let paid_signer = BlindSigner::derive(&[0u8; 32], 0, 0);

        let mock_mm = Arc::new(mock_mm);
        let state = State {
            db: db.clone(),
            mm: mock_mm,
            secp: Secp256k1::new(),
            nostr,
            free_pk: free_signer.pk,
            paid_pk: paid_signer.pk,
            domain: "http://127.0.0.1:8080".to_string(),
            nostr_sk,
        };

        // generate valid blinded message
        let msg = tbs::Message::from_bytes(b"register_username_tests");
        let blinding_key = BlindingKey::random();
        let blinded_msg = blind_message(msg, blinding_key);
        let blind_sig = paid_signer.blind_sign(blinded_msg);
        let sig = unblind_signature(blinding_key, blind_sig);

        let connect = InviteCode::new(
            "ws://test1".parse().unwrap(),
            PeerId::from_str("1").unwrap(),
            FederationId::dummy(),
        );
        let req = RegisterRequest {
            name: Some("registername".to_string()),
            pubkey: "552a9d06810f306bfc085cb1e1c26102554138a51fa3a7fdf98f5b03a945143a".to_string(),
            federation_invite_code: connect.to_string(),
            msg,
            sig,
        };

        match register(&state, req).await {
            Ok(_) => (),
            Err(_) => {
                panic!("shouldn't error")
            }
        }
    }

    #[tokio::test]
    pub async fn register_username_add_unknown_federation_tests() {
        dotenv::dotenv().ok();
        let pg_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let db = setup_db(pg_url);

        // swap out fm with a mock here since that's not what is being tested
        let mut mock_mm = MockMultiMintWrapperTrait::new();
        mock_mm
            .expect_check_has_federation()
            .times(1)
            .returning(|_| false);

        mock_mm
            .expect_register_new_federation()
            .times(1)
            .returning(|_| Ok(()));

        // nostr
        let nostr_nsec_str = std::env::var("NSEC").expect("FM_DB_PATH must be set");
        let nostr_sk = Keys::from_str(&nostr_nsec_str).expect("Invalid NOSTR_SK");
        let nostr = nostr_sdk::Client::new(&nostr_sk);

        // create blind signer
        let free_signer = BlindSigner::derive(&[0u8; 32], 0, 0);
        let paid_signer = BlindSigner::derive(&[0u8; 32], 0, 0);

        let mock_mm = Arc::new(mock_mm);
        let state = State {
            db: db.clone(),
            mm: mock_mm,
            secp: Secp256k1::new(),
            nostr,
            free_pk: free_signer.pk,
            paid_pk: paid_signer.pk,
            domain: "http://127.0.0.1:8080".to_string(),
            nostr_sk,
        };

        // generate valid blinded message
        let signer = BlindSigner::derive(&[0u8; 32], 0, 0);
        let msg = tbs::Message::from_bytes(b"register_username_add_unknown_federation_tests");
        let blinding_key = BlindingKey::random();
        let blinded_msg = blind_message(msg, blinding_key);
        let blind_sig = signer.blind_sign(blinded_msg);
        let sig = unblind_signature(blinding_key, blind_sig);

        let connect = InviteCode::new(
            "ws://test1".parse().unwrap(),
            PeerId::from_str("1").unwrap(),
            FederationId::dummy(),
        );
        let req = RegisterRequest {
            name: Some("newfederationusername".to_string()),
            pubkey: "552a9d06810f306bfc085cb1e1c26102554138a51fa3a7fdf98f5b03a945143a".to_string(),
            federation_invite_code: connect.to_string(),
            msg,
            sig,
        };

        match register(&state, req).await {
            Ok(_) => (),
            Err(_) => {
                panic!("shouldn't error")
            }
        }
    }

    #[tokio::test]
    pub async fn register_username_already_spent_token_tests() {
        dotenv::dotenv().ok();
        let pg_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let db = setup_db(pg_url);

        // swap out fm with a mock here since that's not what is being tested
        let mut mock_mm = MockMultiMintWrapperTrait::new();
        mock_mm
            .expect_check_has_federation()
            .times(1)
            .returning(|_| true);

        // nostr
        let nostr_nsec_str = std::env::var("NSEC").expect("FM_DB_PATH must be set");
        let nostr_sk = Keys::from_str(&nostr_nsec_str).expect("Invalid NOSTR_SK");
        let nostr = nostr_sdk::Client::new(&nostr_sk);

        // create blind signer
        let free_signer = BlindSigner::derive(&[0u8; 32], 0, 0);
        let paid_signer = BlindSigner::derive(&[0u8; 32], 0, 0);

        let mock_mm = Arc::new(mock_mm);
        let state = State {
            db: db.clone(),
            mm: mock_mm,
            secp: Secp256k1::new(),
            nostr,
            free_pk: free_signer.pk,
            paid_pk: paid_signer.pk,
            domain: "http://127.0.0.1:8080".to_string(),
            nostr_sk,
        };

        // generate valid blinded message
        let msg = tbs::Message::from_bytes(b"register_username_already_spent_token_tests");
        let blinding_key = BlindingKey::random();
        let blinded_msg = blind_message(msg, blinding_key);
        let blind_sig = paid_signer.blind_sign(blinded_msg);
        let sig = unblind_signature(blinding_key, blind_sig);

        let connect = InviteCode::new(
            "ws://test1".parse().unwrap(),
            PeerId::from_str("1").unwrap(),
            FederationId::dummy(),
        );
        let req = RegisterRequest {
            name: Some("registername1".to_string()),
            pubkey: "552a9d06810f306bfc085cb1e1c26102554138a51fa3a7fdf98f5b03a945143a".to_string(),
            federation_invite_code: connect.to_string(),
            msg,
            sig,
        };

        // let the first user register sucessfully
        match register(&state, req).await {
            Ok(r) => {
                assert_eq!(r.name, "registername1");
            }
            Err(_) => {
                panic!("shouldn't error")
            }
        }

        // second username attempting to register with the same msg
        // should return the first username it registered with
        let req2 = RegisterRequest {
            name: Some("registername2".to_string()),
            pubkey: "552a9d06810f306bfc085cb1e1c26102554138a51fa3a7fdf98f5b03a945143a".to_string(),
            federation_invite_code: connect.to_string(),
            msg,
            sig,
        };

        match register(&state, req2).await {
            Ok(r) => {
                assert_eq!(r.name, "registername1");
            }
            Err(_) => {
                panic!("shouldn't error")
            }
        }
    }
}
