mod common;

use common::*;
use remit::{kyc, read, transfer, TOKEN_2022};
use solana_program_pack::Pack;
use solana_signer::Signer;
use spl_token_2022_interface::{
    extension::{
        default_account_state::DefaultAccountState,
        metadata_pointer::MetadataPointer,
        mint_close_authority::MintCloseAuthority,
        transfer_fee::{self, TransferFeeAmount},
        BaseStateWithExtensions, ExtensionType, StateWithExtensions,
    },
    instruction::{burn_checked, close_account},
    state::{Account, AccountState, Mint},
};
use spl_token_metadata_interface::state::TokenMetadata;

#[test]
fn mint_has_all_extensions_and_metadata() {
    let mut env = Env::new();
    let issuer = Issuer::new();
    issuer.create(&mut env, false);

    let data = env.data(&issuer.mint());
    let mint = StateWithExtensions::<Mint>::unpack(&data).unwrap();
    let mut exts = mint.get_extension_types().unwrap();
    exts.sort_by_key(|e| *e as u16);
    assert_eq!(
        exts,
        vec![
            ExtensionType::TransferFeeConfig,
            ExtensionType::MintCloseAuthority,
            ExtensionType::DefaultAccountState,
            ExtensionType::MetadataPointer,
            ExtensionType::TokenMetadata,
        ]
    );

    let pointer = mint.get_extension::<MetadataPointer>().unwrap();
    assert_eq!(
        Option::<solana_pubkey::Pubkey>::from(pointer.metadata_address),
        Some(issuer.mint())
    );
    let state = mint.get_extension::<DefaultAccountState>().unwrap();
    assert_eq!(state.state, AccountState::Frozen as u8);
    let close = mint.get_extension::<MintCloseAuthority>().unwrap();
    assert_eq!(
        Option::<solana_pubkey::Pubkey>::from(close.close_authority),
        Some(issuer.close_authority.pubkey())
    );

    let meta = mint.get_variable_len_extension::<TokenMetadata>().unwrap();
    assert_eq!(meta.symbol, "RUSD");
    assert_eq!(meta.mint, issuer.mint());
}

#[test]
fn raw_unpack_does_not_work_on_token_2022_mint() {
    let mut env = Env::new();
    let issuer = Issuer::new();
    issuer.create(&mut env, false);
    let data = env.data(&issuer.mint());
    assert!(Mint::unpack(&data).is_err());
    assert!(StateWithExtensions::<Mint>::unpack(&data).is_ok());
}

#[test]
fn extension_init_after_initialize_mint_fails() {
    let mut env = Env::new();
    let issuer = Issuer::new();
    issuer.create(&mut env, false);
    let ix = spl_token_2022_interface::instruction::initialize_permanent_delegate(
        &TOKEN_2022,
        &issuer.mint(),
        &issuer.seizer.pubkey(),
    )
    .unwrap();
    assert!(env.send(&[ix], &[]).is_err());
}

#[test]
fn new_accounts_start_frozen() {
    let mut env = Env::new();
    let issuer = Issuer::new();
    issuer.create(&mut env, false);
    let user = env.wallet();
    let acc = issuer.open_account(&mut env, &user.pubkey());
    assert!(read::token_account(&env.data(&acc)).unwrap().frozen);
}

#[test]
fn kyc_thaws_one_account_and_leaves_the_default_alone() {
    let mut env = Env::new();
    let issuer = Issuer::new();
    issuer.create(&mut env, false);
    let alice = env.wallet();
    let bob = env.wallet();
    let a = issuer.open_account(&mut env, &alice.pubkey());
    let b = issuer.open_account(&mut env, &bob.pubkey());

    issuer.kyc(&mut env, &a);

    assert!(!read::token_account(&env.data(&a)).unwrap().frozen);
    assert!(read::token_account(&env.data(&b)).unwrap().frozen);
    let data = env.data(&issuer.mint());
    let mint = StateWithExtensions::<Mint>::unpack(&data).unwrap();
    assert_eq!(
        mint.get_extension::<DefaultAccountState>().unwrap().state,
        AccountState::Frozen as u8
    );

    let carol = env.wallet();
    let c = issuer.open_account(&mut env, &carol.pubkey());
    assert!(read::token_account(&env.data(&c)).unwrap().frozen);
}

#[test]
fn only_freeze_authority_can_thaw() {
    let mut env = Env::new();
    let issuer = Issuer::new();
    issuer.create(&mut env, false);
    let user = env.wallet();
    let acc = issuer.open_account(&mut env, &user.pubkey());
    let ix = kyc::clear_kyc(&acc, &issuer.mint(), &user.pubkey());
    assert!(env.send(&[ix], &[&user]).is_err());
}

#[test]
fn frozen_account_cannot_receive() {
    let mut env = Env::new();
    let issuer = Issuer::new();
    issuer.create(&mut env, false);
    let alice = env.wallet();
    let bob = env.wallet();
    let a = issuer.onboard(&mut env, &alice.pubkey(), 10_000);
    let b = issuer.open_account(&mut env, &bob.pubkey());

    let ix = transfer::transfer_ix(
        &env.data(&issuer.mint()),
        env.epoch(),
        &issuer.mint(),
        &a,
        &b,
        &alice.pubkey(),
        1_000,
    )
    .unwrap();
    assert!(env.send(&[ix], &[&alice]).is_err());
}

#[test]
fn transfer_takes_the_fee() {
    let mut env = Env::new();
    let issuer = Issuer::new();
    issuer.create(&mut env, false);
    let alice = env.wallet();
    let bob = env.wallet();
    let a = issuer.onboard(&mut env, &alice.pubkey(), 100_000);
    let b = issuer.onboard(&mut env, &bob.pubkey(), 0);

    let mint_data = env.data(&issuer.mint());
    assert_eq!(
        transfer::expected_fee(&mint_data, env.epoch(), 20_000).unwrap(),
        100
    );

    let ix = transfer::transfer_ix(
        &mint_data,
        env.epoch(),
        &issuer.mint(),
        &a,
        &b,
        &alice.pubkey(),
        20_000,
    )
    .unwrap();
    env.send(&[ix], &[&alice]).unwrap();

    assert_eq!(balance(&env, &a), 80_000);
    assert_eq!(balance(&env, &b), 19_900);
    let data = env.data(&b);
    let acc = StateWithExtensions::<Account>::unpack(&data).unwrap();
    assert_eq!(
        u64::from(
            acc.get_extension::<TransferFeeAmount>()
                .unwrap()
                .withheld_amount
        ),
        100
    );
}

#[test]
fn fee_is_capped() {
    let mut env = Env::new();
    let issuer = Issuer::new();
    issuer.create(&mut env, false);
    let alice = env.wallet();
    let bob = env.wallet();
    let a = issuer.onboard(&mut env, &alice.pubkey(), 10_000_000);
    let b = issuer.onboard(&mut env, &bob.pubkey(), 0);

    let ix = transfer::transfer_ix(
        &env.data(&issuer.mint()),
        env.epoch(),
        &issuer.mint(),
        &a,
        &b,
        &alice.pubkey(),
        5_000_000,
    )
    .unwrap();
    env.send(&[ix], &[&alice]).unwrap();
    assert_eq!(balance(&env, &b), 5_000_000 - MAX_FEE);
}

#[test]
fn cached_fee_breaks_after_rate_change_but_epoch_fee_works() {
    let mut env = Env::new();
    let issuer = Issuer::new();
    issuer.create(&mut env, false);
    let alice = env.wallet();
    let bob = env.wallet();
    let a = issuer.onboard(&mut env, &alice.pubkey(), 100_000);
    let b = issuer.onboard(&mut env, &bob.pubkey(), 0);

    let cached_fee =
        transfer::expected_fee(&env.data(&issuer.mint()), env.epoch(), 10_000).unwrap();

    let ix = transfer_fee::instruction::set_transfer_fee(
        &TOKEN_2022,
        &issuer.mint(),
        &issuer.fee_authority.pubkey(),
        &[],
        100,
        MAX_FEE,
    )
    .unwrap();
    env.send(&[ix], &[&issuer.fee_authority]).unwrap();
    let epoch = env.epoch() + 2;
    env.set_epoch(epoch);

    let stale = transfer_fee::instruction::transfer_checked_with_fee(
        &TOKEN_2022,
        &a,
        &issuer.mint(),
        &b,
        &alice.pubkey(),
        &[],
        10_000,
        DECIMALS,
        cached_fee,
    )
    .unwrap();
    assert!(env.send(&[stale], &[&alice]).is_err());

    let mint_data = env.data(&issuer.mint());
    assert_eq!(
        transfer::expected_fee(&mint_data, env.epoch(), 10_000).unwrap(),
        100
    );
    let ix = transfer::transfer_ix(
        &mint_data,
        env.epoch(),
        &issuer.mint(),
        &a,
        &b,
        &alice.pubkey(),
        10_000,
    )
    .unwrap();
    env.send(&[ix], &[&alice]).unwrap();
    assert_eq!(balance(&env, &b), 9_900);
}

#[test]
fn mint_closes_only_when_supply_is_zero() {
    let mut env = Env::new();
    let issuer = Issuer::new();
    issuer.create(&mut env, false);
    let user = env.wallet();
    let acc = issuer.onboard(&mut env, &user.pubkey(), 5_000);

    let close = close_account(
        &TOKEN_2022,
        &issuer.mint(),
        &env.payer.pubkey(),
        &issuer.close_authority.pubkey(),
        &[],
    )
    .unwrap();
    assert!(env
        .send(std::slice::from_ref(&close), &[&issuer.close_authority])
        .is_err());

    let burn = burn_checked(
        &TOKEN_2022,
        &acc,
        &issuer.mint(),
        &user.pubkey(),
        &[],
        5_000,
        DECIMALS,
    )
    .unwrap();
    env.send(&[burn], &[&user]).unwrap();
    env.send(&[close], &[&issuer.close_authority]).unwrap();
    assert!(!env.exists(&issuer.mint()));
}
