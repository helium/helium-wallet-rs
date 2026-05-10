#![allow(clippy::too_many_arguments)]
use anchor_lang::prelude::*;

pub const TOKEN_METADATA_PROGRAM_ID: Pubkey =
    pubkey!("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");

pub const SPL_NOOP_PROGRAM_ID: Pubkey = pubkey!("noopb9bkMVfRPU8AsbpTUg8AQkHtKwMYZiFUjNRtMmV");

/// Treasury management program ID, hardcoded because `declare_program!` can't
/// digest its IDL (cross-program type reference to `circuit_breaker::state::
/// ThresholdType`). We can still recognize the program in inspect output even
/// without typed instruction decoding.
pub const TREASURY_MANAGEMENT_PROGRAM_ID: Pubkey =
    pubkey!("treaf4wWBBty3fHdyBpo35Mz84M8k3heKXmjmi9vFt5");

declare_program!(helium_sub_daos);
declare_program!(lazy_distributor);
declare_program!(circuit_breaker);
declare_program!(helium_entity_manager);
declare_program!(data_credits);
declare_program!(hexboosting);
declare_program!(rewards_oracle);
declare_program!(spl_account_compression);
declare_program!(bubblegum);
declare_program!(mobile_entity_manager);
declare_program!(price_oracle);
declare_program!(hpl_crons);
declare_program!(squads_mpl);
declare_program!(voter_stake_registry);
declare_program!(fanout);
declare_program!(lazy_transactions);
declare_program!(welcome_pack);

/// A program we recognize by pubkey: HPL programs we generate types for, the
/// Squads multisig programs (v3 and v4), and the most common Solana platform
/// programs that show up in transaction decoding.
///
/// Used by transaction inspectors and instruction decoders to map a raw
/// program ID to a structured identity. Variants serialize to snake_case
/// strings (`"helium_sub_daos"`, `"squads_v4"`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KnownProgram {
    HeliumSubDaos,
    LazyDistributor,
    CircuitBreaker,
    HeliumEntityManager,
    DataCredits,
    Hexboosting,
    RewardsOracle,
    SplAccountCompression,
    Bubblegum,
    MobileEntityManager,
    PriceOracle,
    HplCrons,
    VoterStakeRegistry,
    Fanout,
    LazyTransactions,
    TreasuryManagement,
    WelcomePack,
    /// Squads v3 (`SMPLecH…`).
    SquadsMpl,
    /// Squads v4 (`SQDS4ep…`). The upstream crate calls this
    /// `squads_multisig_program`; we alias to a friendlier name.
    SquadsV4,
    SplToken,
    SplAssociatedToken,
    SystemProgram,
    ComputeBudget,
    AddressLookupTable,
    BpfLoaderUpgradeable,
    TokenMetadata,
    SplNoop,
}

impl KnownProgram {
    /// All variants in declaration order. Useful for iteration and for
    /// building forward lookup tables.
    pub const ALL: &'static [Self] = &[
        Self::HeliumSubDaos,
        Self::LazyDistributor,
        Self::CircuitBreaker,
        Self::HeliumEntityManager,
        Self::DataCredits,
        Self::Hexboosting,
        Self::RewardsOracle,
        Self::SplAccountCompression,
        Self::Bubblegum,
        Self::MobileEntityManager,
        Self::PriceOracle,
        Self::HplCrons,
        Self::VoterStakeRegistry,
        Self::Fanout,
        Self::LazyTransactions,
        Self::TreasuryManagement,
        Self::WelcomePack,
        Self::SquadsMpl,
        Self::SquadsV4,
        Self::SplToken,
        Self::SplAssociatedToken,
        Self::SystemProgram,
        Self::ComputeBudget,
        Self::AddressLookupTable,
        Self::BpfLoaderUpgradeable,
        Self::TokenMetadata,
        Self::SplNoop,
    ];

    /// Solana program ID for this program.
    pub fn id(&self) -> Pubkey {
        match self {
            Self::HeliumSubDaos => helium_sub_daos::ID,
            Self::LazyDistributor => lazy_distributor::ID,
            Self::CircuitBreaker => circuit_breaker::ID,
            Self::HeliumEntityManager => helium_entity_manager::ID,
            Self::DataCredits => data_credits::ID,
            Self::Hexboosting => hexboosting::ID,
            Self::RewardsOracle => rewards_oracle::ID,
            Self::SplAccountCompression => spl_account_compression::ID,
            Self::Bubblegum => bubblegum::ID,
            Self::MobileEntityManager => mobile_entity_manager::ID,
            Self::PriceOracle => price_oracle::ID,
            Self::HplCrons => hpl_crons::ID,
            Self::VoterStakeRegistry => voter_stake_registry::ID,
            Self::Fanout => fanout::ID,
            Self::LazyTransactions => lazy_transactions::ID,
            Self::TreasuryManagement => TREASURY_MANAGEMENT_PROGRAM_ID,
            Self::WelcomePack => welcome_pack::ID,
            Self::SquadsMpl => squads_mpl::ID,
            Self::SquadsV4 => squads_multisig_program::ID,
            Self::SplToken => anchor_spl::token::ID,
            Self::SplAssociatedToken => anchor_spl::associated_token::ID,
            Self::SystemProgram => solana_sdk::system_program::ID,
            Self::ComputeBudget => solana_sdk::compute_budget::ID,
            Self::AddressLookupTable => solana_sdk::address_lookup_table::program::ID,
            Self::BpfLoaderUpgradeable => solana_sdk::bpf_loader_upgradeable::ID,
            Self::TokenMetadata => TOKEN_METADATA_PROGRAM_ID,
            Self::SplNoop => SPL_NOOP_PROGRAM_ID,
        }
    }

    /// Snake-cased name suitable for display or JSON output.
    pub fn name(&self) -> &'static str {
        match self {
            Self::HeliumSubDaos => "helium_sub_daos",
            Self::LazyDistributor => "lazy_distributor",
            Self::CircuitBreaker => "circuit_breaker",
            Self::HeliumEntityManager => "helium_entity_manager",
            Self::DataCredits => "data_credits",
            Self::Hexboosting => "hexboosting",
            Self::RewardsOracle => "rewards_oracle",
            Self::SplAccountCompression => "spl_account_compression",
            Self::Bubblegum => "bubblegum",
            Self::MobileEntityManager => "mobile_entity_manager",
            Self::PriceOracle => "price_oracle",
            Self::HplCrons => "hpl_crons",
            Self::VoterStakeRegistry => "voter_stake_registry",
            Self::Fanout => "fanout",
            Self::LazyTransactions => "lazy_transactions",
            Self::TreasuryManagement => "treasury_management",
            Self::WelcomePack => "welcome_pack",
            Self::SquadsMpl => "squads_mpl",
            Self::SquadsV4 => "squads_v4",
            Self::SplToken => "spl_token",
            Self::SplAssociatedToken => "spl_associated_token",
            Self::SystemProgram => "system_program",
            Self::ComputeBudget => "compute_budget",
            Self::AddressLookupTable => "address_lookup_table",
            Self::BpfLoaderUpgradeable => "bpf_loader_upgradeable",
            Self::TokenMetadata => "token_metadata",
            Self::SplNoop => "spl_noop",
        }
    }

    /// Reverse lookup: given a program ID, return the variant that matches,
    /// or `None` if we don't recognize it.
    pub fn from_pubkey(pubkey: &Pubkey) -> Option<Self> {
        Self::ALL.iter().copied().find(|kp| kp.id() == *pubkey)
    }

    /// Resolve a snake-cased instruction method name from an Anchor 8-byte
    /// discriminator, when this program ships a parseable IDL. Returns
    /// `None` for programs without an IDL (Squads v3/v4, SPL Token, etc.)
    /// or instructions not present in the IDL. Anchor framework
    /// instructions (e.g. on-chain IDL management) are recognized for any
    /// program in the registry.
    pub fn method_name(&self, discriminator: &[u8; 8]) -> Option<&'static str> {
        idl::method_name(*self, discriminator)
    }

    /// Like `method_name`, but with access to the instruction body so the
    /// returned name can include the Anchor IDL sub-op when applicable
    /// (e.g. `"anchor:idl_set_buffer"` instead of just `"anchor:idl"`).
    pub fn method_name_with_body(
        &self,
        discriminator: &[u8; 8],
        body: &[u8],
    ) -> Option<&'static str> {
        idl::method_name_with_body(*self, discriminator, body)
    }

    /// True for programs whose IDL we ship and parse. Lets callers
    /// distinguish "method unknown because we don't decode this program"
    /// from "method unknown despite having the IDL" — the latter signals
    /// either a stale IDL or a since-removed instruction.
    pub fn has_idl(&self) -> bool {
        idl::has_idl(*self)
    }

    /// Decode an Anchor instruction's args by walking this program's IDL
    /// against the borsh-encoded body (everything after the 8-byte
    /// discriminator). Returns `None` if the discriminator isn't in the
    /// IDL or if any field fails to decode (truncated bytes, unsupported
    /// type, etc.). Output is structured JSON with field names from the
    /// IDL; consumers serialize it as part of their own response shape.
    pub fn decode_instruction_args(
        &self,
        discriminator: &[u8; 8],
        body: &[u8],
    ) -> Option<serde_json::Value> {
        idl::decode_instruction_args(*self, discriminator, body)
    }
}

/// Discriminator → method-name resolution and IDL-driven Borsh arg decoding,
/// backed by the embedded HPL IDL JSONs in `helium-lib/idls/`. Each program's
/// IDL is parsed once on first use and held for process lifetime.
mod idl {
    use super::KnownProgram;
    use serde_json::{Map, Value};
    use solana_sdk::pubkey::Pubkey;
    use std::{collections::HashMap, sync::LazyLock};

    /// One row per HPL program with an IDL we can decode. Programs whose
    /// IDLs we don't ship — Squads v3/v4, SPL Token, etc. — simply aren't
    /// in this list.
    const IDLS: &[(KnownProgram, &str)] = &[
        (
            KnownProgram::HeliumSubDaos,
            include_str!("../idls/helium_sub_daos.json"),
        ),
        (
            KnownProgram::LazyDistributor,
            include_str!("../idls/lazy_distributor.json"),
        ),
        (
            KnownProgram::CircuitBreaker,
            include_str!("../idls/circuit_breaker.json"),
        ),
        (
            KnownProgram::HeliumEntityManager,
            include_str!("../idls/helium_entity_manager.json"),
        ),
        (
            KnownProgram::DataCredits,
            include_str!("../idls/data_credits.json"),
        ),
        (
            KnownProgram::Hexboosting,
            include_str!("../idls/hexboosting.json"),
        ),
        (
            KnownProgram::RewardsOracle,
            include_str!("../idls/rewards_oracle.json"),
        ),
        (
            KnownProgram::SplAccountCompression,
            include_str!("../idls/spl_account_compression.json"),
        ),
        (
            KnownProgram::Bubblegum,
            include_str!("../idls/bubblegum.json"),
        ),
        (
            KnownProgram::MobileEntityManager,
            include_str!("../idls/mobile_entity_manager.json"),
        ),
        (
            KnownProgram::PriceOracle,
            include_str!("../idls/price_oracle.json"),
        ),
        (
            KnownProgram::HplCrons,
            include_str!("../idls/hpl_crons.json"),
        ),
        (
            KnownProgram::VoterStakeRegistry,
            include_str!("../idls/voter_stake_registry.json"),
        ),
        (KnownProgram::Fanout, include_str!("../idls/fanout.json")),
        (
            KnownProgram::LazyTransactions,
            include_str!("../idls/lazy_transactions.json"),
        ),
        (
            KnownProgram::WelcomePack,
            include_str!("../idls/welcome_pack.json"),
        ),
    ];

    /// Parsed view of one program's IDL: instruction dispatch table by
    /// discriminator, plus the named-type table needed to decode `defined`
    /// references inside instruction args.
    pub(super) struct ProgramIdl {
        pub instructions: HashMap<[u8; 8], Instruction>,
        pub types: HashMap<String, TypeDef>,
    }

    pub(super) struct Instruction {
        pub name: String,
        pub args: Vec<Field>,
    }

    pub(super) struct Field {
        pub name: String,
        pub ty: Type,
    }

    pub(super) enum TypeDef {
        Struct(Vec<Field>),
        Enum(Vec<EnumVariant>),
    }

    pub(super) struct EnumVariant {
        pub name: String,
        pub fields: VariantFields,
    }

    pub(super) enum VariantFields {
        Unit,
        Named(Vec<Field>),
        Tuple(Vec<Type>),
    }

    /// Anchor IDL types we can decode. Anchor's full type set is broader
    /// (HashMap, BTreeMap, generics, etc.) but Helium IDLs use only the
    /// subset listed here. Unrecognized types fall through to `None`.
    pub(super) enum Type {
        Bool,
        U8,
        U16,
        U32,
        U64,
        U128,
        I8,
        I16,
        I32,
        I64,
        I128,
        F32,
        F64,
        String,
        Bytes,
        Pubkey,
        Vec(Box<Type>),
        Option(Box<Type>),
        Array(Box<Type>, usize),
        Defined(String),
    }

    static IDL_DATA: LazyLock<HashMap<KnownProgram, ProgramIdl>> = LazyLock::new(|| {
        IDLS.iter()
            .map(|(program, json)| (*program, parse(json)))
            .collect()
    });

    fn parse(idl: &str) -> ProgramIdl {
        // IDLs are bundled via include_str! at compile time. A parse
        // failure here means the shipped IDL is malformed — refresh
        // it via gen_idl.sh rather than letting every method/arg
        // lookup silently inflate `unknown_methods`.
        let v: Value = serde_json::from_str(idl)
            .expect("compile-time-bundled IDL must parse — refresh via gen_idl.sh");
        let instructions = v
            .get("instructions")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|inst| {
                        let name = inst.get("name")?.as_str()?.to_owned();
                        let disc = inst.get("discriminator")?.as_array()?;
                        let bytes: Vec<u8> = disc
                            .iter()
                            .filter_map(|v| u8::try_from(v.as_u64()?).ok())
                            .collect();
                        let key: [u8; 8] = bytes.try_into().ok()?;
                        let args = inst
                            .get("args")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(parse_field).collect())
                            .unwrap_or_default();
                        Some((key, Instruction { name, args }))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let types = v
            .get("types")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|td| {
                        let name = td.get("name")?.as_str()?.to_owned();
                        let body = td.get("type")?;
                        let kind = body.get("kind")?.as_str()?;
                        let parsed = match kind {
                            "struct" => TypeDef::Struct(
                                body.get("fields")
                                    .and_then(Value::as_array)?
                                    .iter()
                                    .filter_map(parse_field)
                                    .collect(),
                            ),
                            "enum" => TypeDef::Enum(
                                body.get("variants")
                                    .and_then(Value::as_array)?
                                    .iter()
                                    .filter_map(parse_enum_variant)
                                    .collect(),
                            ),
                            _ => return None,
                        };
                        Some((name, parsed))
                    })
                    .collect()
            })
            .unwrap_or_default();
        ProgramIdl {
            instructions,
            types,
        }
    }

    fn parse_field(v: &Value) -> Option<Field> {
        Some(Field {
            name: v.get("name")?.as_str()?.to_owned(),
            ty: parse_type(v.get("type")?)?,
        })
    }

    fn parse_enum_variant(v: &Value) -> Option<EnumVariant> {
        let name = v.get("name")?.as_str()?.to_owned();
        let fields = match v.get("fields") {
            None => VariantFields::Unit,
            Some(Value::Array(arr)) => {
                // Two shapes: array of {name, type} for named-field
                // variants, or array of bare types for tuple variants.
                if arr.iter().all(|f| f.get("name").is_some()) {
                    VariantFields::Named(arr.iter().filter_map(parse_field).collect())
                } else {
                    VariantFields::Tuple(arr.iter().filter_map(parse_type).collect())
                }
            }
            _ => VariantFields::Unit,
        };
        Some(EnumVariant { name, fields })
    }

    fn parse_type(v: &Value) -> Option<Type> {
        if let Some(prim) = v.as_str() {
            return primitive_type(prim);
        }
        if let Some(obj) = v.as_object() {
            if let Some(inner) = obj.get("vec") {
                return Some(Type::Vec(Box::new(parse_type(inner)?)));
            }
            if let Some(inner) = obj.get("option") {
                return Some(Type::Option(Box::new(parse_type(inner)?)));
            }
            if let Some(arr) = obj.get("array").and_then(Value::as_array) {
                if arr.len() == 2 {
                    let inner = parse_type(&arr[0])?;
                    let n = arr[1].as_u64()? as usize;
                    return Some(Type::Array(Box::new(inner), n));
                }
            }
            if let Some(def) = obj.get("defined") {
                let name = def.get("name")?.as_str()?.to_owned();
                return Some(Type::Defined(name));
            }
        }
        None
    }

    fn primitive_type(s: &str) -> Option<Type> {
        Some(match s {
            "bool" => Type::Bool,
            "u8" => Type::U8,
            "u16" => Type::U16,
            "u32" => Type::U32,
            "u64" => Type::U64,
            "u128" => Type::U128,
            "i8" => Type::I8,
            "i16" => Type::I16,
            "i32" => Type::I32,
            "i64" => Type::I64,
            "i128" => Type::I128,
            "f32" => Type::F32,
            "f64" => Type::F64,
            "string" => Type::String,
            "bytes" => Type::Bytes,
            "pubkey" | "publicKey" => Type::Pubkey,
            _ => return None,
        })
    }

    /// Anchor's framework-level IDL management instruction tag —
    /// `sha256("anchor:idl")[..8]` interpreted as a little-endian u64.
    /// Every Anchor program responds to this discriminator with its
    /// built-in IDL ops (init, write, set_authority, set_buffer, close,
    /// resize). The byte immediately after the tag selects which sub-op.
    /// Showing up as a sibling instruction during program-upgrade
    /// ceremonies is normal — devs upgrade the on-chain IDL alongside
    /// the program binary.
    const ANCHOR_IDL_IX_TAG: [u8; 8] = [64, 244, 188, 120, 167, 233, 105, 10];

    fn anchor_idl_method_name(body: &[u8]) -> &'static str {
        // Sub-op order matches anchor-lang's `IdlInstruction` enum.
        match body.first() {
            Some(0) => "anchor:idl_create",
            Some(1) => "anchor:idl_create_buffer",
            Some(2) => "anchor:idl_write",
            Some(3) => "anchor:idl_set_authority",
            Some(4) => "anchor:idl_set_buffer",
            Some(5) => "anchor:idl_close",
            Some(6) => "anchor:idl_resize",
            _ => "anchor:idl",
        }
    }

    pub(super) fn method_name(
        program: KnownProgram,
        discriminator: &[u8; 8],
    ) -> Option<&'static str> {
        // Framework-level IDL management is independent of any program's
        // own instruction set — recognize it across every Anchor program,
        // including those whose IDL we don't ship. Check this first so
        // IDL-less programs still pick it up.
        if *discriminator == ANCHOR_IDL_IX_TAG {
            return Some("anchor:idl");
        }
        // The strings live inside the LazyLock, which is itself 'static —
        // so the returned &str borrow is also 'static.
        IDL_DATA
            .get(&program)?
            .instructions
            .get(discriminator)
            .map(|i| i.name.as_str())
    }

    /// Like `method_name` but with access to the full instruction body so
    /// the returned name can include the IDL sub-op when applicable
    /// (e.g. `"anchor:idl_set_buffer"`). Falls back to `method_name` for
    /// regular Anchor user instructions.
    pub(super) fn method_name_with_body(
        program: KnownProgram,
        discriminator: &[u8; 8],
        body: &[u8],
    ) -> Option<&'static str> {
        if *discriminator == ANCHOR_IDL_IX_TAG {
            return Some(anchor_idl_method_name(body));
        }
        method_name(program, discriminator)
    }

    pub(super) fn has_idl(program: KnownProgram) -> bool {
        IDLS.iter().any(|(p, _)| *p == program)
    }

    pub(super) fn decode_instruction_args(
        program: KnownProgram,
        discriminator: &[u8; 8],
        body: &[u8],
    ) -> Option<Value> {
        let idl = IDL_DATA.get(&program)?;
        let inst = idl.instructions.get(discriminator)?;
        let mut cursor = body;
        let mut obj = Map::new();
        for field in &inst.args {
            let value = decode_value(&field.ty, &mut cursor, idl)?;
            obj.insert(field.name.clone(), value);
        }
        Some(Value::Object(obj))
    }

    fn decode_value(ty: &Type, data: &mut &[u8], idl: &ProgramIdl) -> Option<Value> {
        Some(match ty {
            Type::Bool => Value::Bool(read_u8(data)? != 0),
            Type::U8 => Value::from(read_u8(data)?),
            Type::U16 => Value::from(read_u16(data)?),
            Type::U32 => Value::from(read_u32(data)?),
            Type::U64 => Value::from(read_u64(data)?),
            Type::U128 => Value::String(read_u128(data)?.to_string()),
            Type::I8 => Value::from(read_i8(data)?),
            Type::I16 => Value::from(read_i16(data)?),
            Type::I32 => Value::from(read_i32(data)?),
            Type::I64 => Value::from(read_i64(data)?),
            Type::I128 => Value::String(read_i128(data)?.to_string()),
            Type::F32 => {
                let bytes = take(data, 4)?;
                Value::from(f64::from(f32::from_le_bytes(bytes.try_into().ok()?)))
            }
            Type::F64 => {
                let bytes = take(data, 8)?;
                Value::from(f64::from_le_bytes(bytes.try_into().ok()?))
            }
            Type::String => {
                let len = read_u32(data)? as usize;
                let bytes = take(data, len)?;
                Value::String(String::from_utf8(bytes.to_vec()).ok()?)
            }
            Type::Bytes => {
                let len = read_u32(data)? as usize;
                let bytes = take(data, len)?;
                Value::String(solana_sdk::bs58::encode(bytes).into_string())
            }
            Type::Pubkey => {
                let bytes = take(data, 32)?;
                let arr: [u8; 32] = bytes.try_into().ok()?;
                Value::String(Pubkey::from(arr).to_string())
            }
            Type::Vec(inner) => {
                let len = read_u32(data)? as usize;
                let mut arr = Vec::with_capacity(len);
                for _ in 0..len {
                    arr.push(decode_value(inner, data, idl)?);
                }
                Value::Array(arr)
            }
            Type::Option(inner) => match read_u8(data)? {
                0 => Value::Null,
                _ => decode_value(inner, data, idl)?,
            },
            Type::Array(inner, n) => {
                let mut arr = Vec::with_capacity(*n);
                for _ in 0..*n {
                    arr.push(decode_value(inner, data, idl)?);
                }
                Value::Array(arr)
            }
            Type::Defined(name) => decode_typedef(idl.types.get(name)?, data, idl)?,
        })
    }

    fn decode_typedef(def: &TypeDef, data: &mut &[u8], idl: &ProgramIdl) -> Option<Value> {
        match def {
            TypeDef::Struct(fields) => {
                let mut obj = Map::new();
                for field in fields {
                    obj.insert(field.name.clone(), decode_value(&field.ty, data, idl)?);
                }
                Some(Value::Object(obj))
            }
            TypeDef::Enum(variants) => {
                let tag = read_u8(data)? as usize;
                let variant = variants.get(tag)?;
                let mut obj = Map::new();
                obj.insert("type".into(), Value::String(variant.name.clone()));
                match &variant.fields {
                    VariantFields::Unit => {}
                    VariantFields::Named(fields) => {
                        for field in fields {
                            obj.insert(field.name.clone(), decode_value(&field.ty, data, idl)?);
                        }
                    }
                    VariantFields::Tuple(types) => {
                        let mut arr = Vec::with_capacity(types.len());
                        for ty in types {
                            arr.push(decode_value(ty, data, idl)?);
                        }
                        obj.insert("values".into(), Value::Array(arr));
                    }
                }
                Some(Value::Object(obj))
            }
        }
    }

    fn take<'a>(data: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
        if data.len() < n {
            return None;
        }
        let (head, tail) = data.split_at(n);
        *data = tail;
        Some(head)
    }

    macro_rules! read_int {
        ($name:ident, $ty:ty, $n:expr) => {
            fn $name(data: &mut &[u8]) -> Option<$ty> {
                let bytes = take(data, $n)?;
                Some(<$ty>::from_le_bytes(bytes.try_into().ok()?))
            }
        };
    }
    read_int!(read_u8, u8, 1);
    read_int!(read_u16, u16, 2);
    read_int!(read_u32, u32, 4);
    read_int!(read_u64, u64, 8);
    read_int!(read_u128, u128, 16);
    read_int!(read_i8, i8, 1);
    read_int!(read_i16, i16, 2);
    read_int!(read_i32, i32, 4);
    read_int!(read_i64, i64, 8);
    read_int!(read_i128, i128, 16);

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Sanity check that every IDL we ship parses into a non-empty
        /// instruction table — guards against a future IDL regen producing
        /// JSON we can't read.
        #[test]
        fn every_idl_yields_methods() {
            for (program, _) in IDLS {
                let idl = IDL_DATA
                    .get(program)
                    .unwrap_or_else(|| panic!("no IDL parsed for {program:?}"));
                assert!(
                    !idl.instructions.is_empty(),
                    "{program:?} parsed to an empty instruction table"
                );
            }
        }

        /// Spot-check one known instruction round-trip from the IDL.
        #[test]
        fn helium_sub_daos_resolves_known_instruction() {
            assert_eq!(
                method_name(
                    KnownProgram::HeliumSubDaos,
                    &[64, 233, 120, 46, 172, 83, 84, 163],
                ),
                Some("add_recent_proposal_to_dao_v0"),
            );
        }

        /// Decode a tiny synthetic args body and confirm field names + values
        /// come back as expected. Covers primitives + Option + Pubkey end to
        /// end without depending on a specific real-world instruction.
        #[test]
        fn decoder_handles_primitives_and_option() {
            let mut body: Vec<u8> = Vec::new();
            body.extend_from_slice(&42u64.to_le_bytes()); // u64
            body.push(1); // Option<u8> = Some(7)
            body.push(7);
            body.extend_from_slice(&[5u8; 32]); // Pubkey
            let idl = ProgramIdl {
                instructions: HashMap::new(),
                types: HashMap::new(),
            };
            let fields = vec![
                Field {
                    name: "amount".into(),
                    ty: Type::U64,
                },
                Field {
                    name: "maybe".into(),
                    ty: Type::Option(Box::new(Type::U8)),
                },
                Field {
                    name: "key".into(),
                    ty: Type::Pubkey,
                },
            ];
            let mut cursor = body.as_slice();
            let mut obj = serde_json::Map::new();
            for f in &fields {
                obj.insert(
                    f.name.clone(),
                    decode_value(&f.ty, &mut cursor, &idl).unwrap(),
                );
            }
            assert_eq!(obj["amount"], Value::from(42u64));
            assert_eq!(obj["maybe"], Value::from(7u8));
            assert_eq!(
                obj["key"].as_str().unwrap(),
                Pubkey::from([5u8; 32]).to_string()
            );
        }

        /// Vec<T> in Borsh is a `u32 LE length + N×T` payload. Verifies
        /// the length-prefix read and per-element decode.
        #[test]
        fn decoder_handles_vec() {
            let mut body: Vec<u8> = Vec::new();
            body.extend_from_slice(&3u32.to_le_bytes());
            body.extend_from_slice(&[1u8, 2, 3]);
            let idl = ProgramIdl {
                instructions: HashMap::new(),
                types: HashMap::new(),
            };
            let mut cursor = body.as_slice();
            let value = decode_value(&Type::Vec(Box::new(Type::U8)), &mut cursor, &idl).unwrap();
            assert_eq!(value, Value::from(vec![1u8, 2, 3]));
        }

        /// Array<T, N> in Borsh has no length prefix — just N elements.
        /// Regression: a swap to "u32 length first" would break here.
        #[test]
        fn decoder_handles_array() {
            let body = vec![10u8, 20, 30, 40];
            let idl = ProgramIdl {
                instructions: HashMap::new(),
                types: HashMap::new(),
            };
            let mut cursor = body.as_slice();
            let value =
                decode_value(&Type::Array(Box::new(Type::U8), 4), &mut cursor, &idl).unwrap();
            assert_eq!(value, Value::from(vec![10u8, 20, 30, 40]));
        }

        /// `Type::Defined` resolves to a struct from the IDL's types map.
        /// Decode the body field-by-field and confirm the JSON shape.
        #[test]
        fn decoder_handles_defined_struct() {
            let mut types = HashMap::new();
            types.insert(
                "Inner".to_string(),
                TypeDef::Struct(vec![
                    Field {
                        name: "a".into(),
                        ty: Type::U8,
                    },
                    Field {
                        name: "b".into(),
                        ty: Type::U16,
                    },
                ]),
            );
            let idl = ProgramIdl {
                instructions: HashMap::new(),
                types,
            };
            let body = vec![7u8, 0xff, 0x00]; // u8=7, u16=255 LE
            let mut cursor = body.as_slice();
            let value =
                decode_value(&Type::Defined("Inner".to_string()), &mut cursor, &idl).unwrap();
            let obj = value.as_object().unwrap();
            assert_eq!(obj["a"], Value::from(7u8));
            assert_eq!(obj["b"], Value::from(255u16));
        }

        /// A truncated body must return `None` — the decode fails silently
        /// at the inspect output (args omitted) rather than panicking or
        /// decoding garbage. Test feeds a 4-byte buffer to a u64 read.
        #[test]
        fn decoder_short_buffer_returns_none() {
            let body = vec![1u8, 2, 3, 4]; // Only 4 bytes, u64 needs 8.
            let idl = ProgramIdl {
                instructions: HashMap::new(),
                types: HashMap::new(),
            };
            let mut cursor = body.as_slice();
            assert!(decode_value(&Type::U64, &mut cursor, &idl).is_none());
        }
    }
}
