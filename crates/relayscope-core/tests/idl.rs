//! Proves the decoders match the pinned IDL: every instruction, event, account, type and error,
//! with identical discriminators, field names, field types and account names.

use std::collections::BTreeSet;

use relayscope_core::shape::{IdlType, Shape, ShapeKind};
use relayscope_core::{ACCOUNTS, EVENTS, INSTRUCTIONS, PROGRAM_ERRORS, PROGRAM_ID, types};
use serde_json::Value;
use sha2::{Digest, Sha256};

const IDL: &str = include_str!("../../../idl/donation_relay.json");

fn idl() -> Value {
    serde_json::from_str(IDL).expect("pinned IDL is JSON")
}

/// Spells an IDL type the way it is written in Rust.
fn rust_type(t: &Value) -> String {
    match t {
        Value::String(s) => match s.as_str() {
            "pubkey" => "Pubkey".into(),
            "string" => "String".into(),
            "bytes" => "Bytes".into(),
            other => other.into(),
        },
        Value::Object(o) if o.contains_key("vec") => format!("Vec<{}>", rust_type(&o["vec"])),
        Value::Object(o) if o.contains_key("option") => {
            format!("Option<{}>", rust_type(&o["option"]))
        }
        Value::Object(o) if o.contains_key("array") => {
            let array = o["array"].as_array().unwrap();
            assert_eq!(
                (rust_type(&array[0]), array[1].as_u64()),
                ("u8".into(), Some(32)),
                "only [u8; 32] arrays are expected"
            );
            "Bytes32".into()
        }
        Value::Object(o) if o.contains_key("defined") => {
            o["defined"]["name"].as_str().unwrap().into()
        }
        other => panic!("unexpected IDL type {other}"),
    }
}

fn idl_fields(fields: &Value) -> Vec<(String, String)> {
    fields
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .map(|f| {
                    (
                        f["name"].as_str().unwrap().to_string(),
                        rust_type(&f["type"]),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn rust_fields(shape: &Shape) -> Vec<(String, String)> {
    match shape.kind {
        ShapeKind::Struct(fields) => fields
            .iter()
            .map(|(name, ty)| (name.to_string(), ty.split_whitespace().collect::<String>()))
            .collect(),
        ShapeKind::Enum(_) => panic!("{} is an enum", shape.name),
    }
}

fn idl_type<'a>(idl: &'a Value, name: &str) -> &'a Value {
    idl["types"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("IDL has no type {name}"))
}

fn discriminator(v: &Value) -> [u8; 8] {
    let bytes: Vec<u8> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap() as u8)
        .collect();
    bytes.try_into().unwrap()
}

fn names(list: &Value) -> BTreeSet<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn pinned_file_matches_its_recorded_digest() {
    let digest = hex::encode(Sha256::digest(IDL.as_bytes()));
    assert!(relayscope_core::program::IDL_SHA256.starts_with(&digest));
}

#[test]
fn program_address_and_version() {
    let idl = idl();
    assert_eq!(idl["address"], PROGRAM_ID.to_string());
    assert_eq!(
        idl["metadata"]["version"],
        relayscope_core::program::IDL_VERSION
    );
}

#[test]
fn instructions_match() {
    let idl = idl();
    let ours: BTreeSet<String> = INSTRUCTIONS.iter().map(|i| i.name.to_string()).collect();
    assert_eq!(ours, names(&idl["instructions"]), "instruction set differs");

    for info in INSTRUCTIONS {
        let ix = idl["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == info.name)
            .unwrap();
        assert_eq!(
            info.discriminator,
            discriminator(&ix["discriminator"]),
            "{} discriminator",
            info.name
        );
        assert_eq!(
            rust_fields(&info.args),
            idl_fields(&ix["args"]),
            "{} arguments",
            info.name
        );
        let accounts: Vec<&str> = ix["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!(info.accounts, accounts.as_slice(), "{} accounts", info.name);
    }
}

#[test]
fn events_match() {
    let idl = idl();
    let ours: BTreeSet<String> = EVENTS.iter().map(|e| e.name.to_string()).collect();
    assert_eq!(ours, names(&idl["events"]), "event set differs");

    for info in EVENTS {
        let event = idl["events"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == info.name)
            .unwrap();
        assert_eq!(
            info.discriminator,
            discriminator(&event["discriminator"]),
            "{} discriminator",
            info.name
        );
        let fields = &idl_type(&idl, info.name)["type"]["fields"];
        assert_eq!(
            rust_fields(&info.shape),
            idl_fields(fields),
            "{} fields",
            info.name
        );
    }
}

#[test]
fn accounts_match() {
    let idl = idl();
    let ours: BTreeSet<String> = ACCOUNTS.iter().map(|a| a.name.to_string()).collect();
    assert_eq!(ours, names(&idl["accounts"]), "account set differs");

    for info in ACCOUNTS {
        let account = idl["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == info.name)
            .unwrap();
        assert_eq!(
            info.discriminator,
            discriminator(&account["discriminator"]),
            "{} discriminator",
            info.name
        );
        let fields = &idl_type(&idl, info.name)["type"]["fields"];
        assert_eq!(
            rust_fields(&info.shape),
            idl_fields(fields),
            "{} fields",
            info.name
        );
    }
}

#[test]
fn shared_types_match() {
    let idl = idl();
    let shapes = [
        types::ShareType::SHAPE,
        types::DaasEventDiscriminator::SHAPE,
        types::Share::SHAPE,
        types::Receipt::SHAPE,
        types::ExactTransfer::SHAPE,
        types::DebouncerV1State::SHAPE,
        types::DistributionV1State::SHAPE,
        types::EpochTrackerV1State::SHAPE,
    ];
    for shape in &shapes {
        let t = &idl_type(&idl, shape.name)["type"];
        match shape.kind {
            ShapeKind::Struct(_) => assert_eq!(
                rust_fields(shape),
                idl_fields(&t["fields"]),
                "{} fields",
                shape.name
            ),
            ShapeKind::Enum(variants) => {
                let idl_variants: Vec<&str> = t["variants"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v["name"].as_str().unwrap())
                    .collect();
                assert_eq!(variants, idl_variants.as_slice(), "{} variants", shape.name);
            }
        }
    }

    // Every IDL type is covered by an event, an account or one of the shared types above.
    let covered: BTreeSet<String> = shapes
        .iter()
        .map(|s| s.name.to_string())
        .chain(EVENTS.iter().map(|e| e.name.to_string()))
        .chain(ACCOUNTS.iter().map(|a| a.name.to_string()))
        .collect();
    assert_eq!(
        covered,
        names(&idl["types"]),
        "IDL types not covered by a Rust type"
    );
}

#[test]
fn errors_match() {
    let idl = idl();
    let idl_errors: Vec<(u64, String, String)> = idl["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["code"].as_u64().unwrap(),
                e["name"].as_str().unwrap().to_string(),
                e["msg"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let ours: Vec<(u64, String, String)> = PROGRAM_ERRORS
        .iter()
        .map(|e| (u64::from(e.code), e.name.to_string(), e.message.to_string()))
        .collect();
    assert_eq!(ours, idl_errors);
}
