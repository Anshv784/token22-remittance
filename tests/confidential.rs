mod common;

use common::*;
use remit::{
    confidential::{self, ProofFlow, UserKeys},
    kyc, read, transfer,
};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use spl_token_2022_interface::{
    extension::{BaseStateWithExtensions, ExtensionType, StateWithExtensions},
    state::Mint,
};

struct User {
    wallet: Keypair,
    keys: UserKeys,
    acc: Pubkey,
}

fn setup() -> (Env, Issuer) {
    let mut env = Env::new();
    let issuer = Issuer::new();
    issuer.create(&mut env, true);
    (env, issuer)
}

fn confidential_user(env: &mut Env, issuer: &Issuer, funds: u64) -> User {
    let wallet = env.wallet();
    let keys = UserKeys::new_rand();
    let acc = issuer.onboard(env, &wallet.pubkey(), funds);
    let ixs = confidential::configure_ixs(
        &env.payer.pubkey(),
        &wallet.pubkey(),
        &acc,
        &issuer.mint(),
        &keys,
    );
    env.send(&ixs, &[&wallet]).unwrap();
    let ix = confidential::approve_ix(&acc, &issuer.mint(), &issuer.ct_authority.pubkey());
    env.send(&[ix], &[&issuer.ct_authority]).unwrap();
    User { wallet, keys, acc }
}

fn deposit(env: &mut Env, issuer: &Issuer, user: &User, amount: u64) -> Result<(), String> {
    let ix = confidential::deposit_ix(
        &user.acc,
        &issuer.mint(),
        &user.wallet.pubkey(),
        amount,
        DECIMALS,
    );
    env.send(&[ix], &[&user.wallet])
}

fn apply_pending(env: &mut Env, user: &User) {
    let ix = confidential::apply_pending_ix(
        &env.data(&user.acc),
        &user.acc,
        &user.wallet.pubkey(),
        &user.keys,
    )
    .unwrap();
    env.send(&[ix], &[&user.wallet]).unwrap();
}

fn run(env: &mut Env, flow: ProofFlow, owner: &Keypair) -> Result<(), String> {
    let creates: Vec<_> = flow.proofs.iter().map(|p| p.create.clone()).collect();
    let keypairs: Vec<&Keypair> = flow.proofs.iter().map(|p| &p.keypair).collect();
    env.send(&creates, &keypairs).unwrap();
    for p in &flow.proofs {
        env.send_heavy(std::slice::from_ref(&p.verify), &[])?;
    }
    let res = env.send_heavy(&flow.token_ixs, &[owner]);
    let closes: Vec<_> = flow.proofs.iter().map(|p| p.close.clone()).collect();
    env.send(&closes, &[owner]).unwrap();
    res
}

fn send_confidential(
    env: &mut Env,
    issuer: &Issuer,
    from: &User,
    to: &User,
    amount: u64,
) -> Result<(), String> {
    let rent = |len| env.rent(len);
    let flow = confidential::transfer_flow(
        &env.payer.pubkey(),
        &from.wallet.pubkey(),
        &issuer.mint(),
        &env.data(&issuer.mint()),
        env.epoch(),
        &from.acc,
        &env.data(&from.acc),
        &to.acc,
        &env.data(&to.acc),
        &from.keys,
        amount,
        &rent,
    )
    .map_err(|e| e.to_string())?;
    run(env, flow, &from.wallet)
}

fn withdraw(env: &mut Env, issuer: &Issuer, user: &User, amount: u64) -> Result<(), String> {
    let rent = |len| env.rent(len);
    let flow = confidential::withdraw_flow(
        &env.payer.pubkey(),
        &user.wallet.pubkey(),
        &issuer.mint(),
        DECIMALS,
        &user.acc,
        &env.data(&user.acc),
        &user.keys,
        amount,
        &rent,
    )
    .map_err(|e| e.to_string())?;
    run(env, flow, &user.wallet)
}

fn seize(
    env: &mut Env,
    issuer: &Issuer,
    from: &Pubkey,
    to: &Pubkey,
    amount: u64,
) -> Result<(), String> {
    let ix = transfer::transfer_ix(
        &env.data(&issuer.mint()),
        env.epoch(),
        &issuer.mint(),
        from,
        to,
        &issuer.seizer.pubkey(),
        amount,
    )
    .unwrap();
    env.send(&[ix], &[&issuer.seizer])
}

#[test]
fn reissued_mint_keeps_the_old_extensions() {
    let (env, issuer) = setup();
    let data = env.data(&issuer.mint());
    let mint = StateWithExtensions::<Mint>::unpack(&data).unwrap();
    let exts = mint.get_extension_types().unwrap();
    for e in [
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
        ExtensionType::MintCloseAuthority,
        ExtensionType::TokenMetadata,
        ExtensionType::PermanentDelegate,
        ExtensionType::ConfidentialTransferMint,
        ExtensionType::ConfidentialTransferFeeConfig,
    ] {
        assert!(exts.contains(&e), "missing {:?}", e);
    }
}

#[test]
fn anyone_can_create_the_ata_but_only_the_owner_can_configure() {
    let (mut env, issuer) = setup();
    let owner = env.wallet();
    let stranger = env.wallet();
    let acc = issuer.open_account(&mut env, &owner.pubkey());
    issuer.kyc(&mut env, &acc);

    let keys = UserKeys::new_rand();
    let ixs = confidential::configure_ixs(
        &env.payer.pubkey(),
        &stranger.pubkey(),
        &acc,
        &issuer.mint(),
        &keys,
    );
    assert!(env.send(&ixs, &[&stranger]).is_err());

    let ixs = confidential::configure_ixs(
        &env.payer.pubkey(),
        &owner.pubkey(),
        &acc,
        &issuer.mint(),
        &keys,
    );
    env.send(&ixs, &[&owner]).unwrap();
}

#[test]
fn deposit_needs_issuer_approval() {
    let (mut env, issuer) = setup();
    let wallet = env.wallet();
    let keys = UserKeys::new_rand();
    let acc = issuer.onboard(&mut env, &wallet.pubkey(), 10_000);
    let ixs = confidential::configure_ixs(
        &env.payer.pubkey(),
        &wallet.pubkey(),
        &acc,
        &issuer.mint(),
        &keys,
    );
    env.send(&ixs, &[&wallet]).unwrap();
    let user = User { wallet, keys, acc };

    assert!(deposit(&mut env, &issuer, &user, 1_000).is_err());

    let ix = confidential::approve_ix(&user.acc, &issuer.mint(), &issuer.ct_authority.pubkey());
    env.send(&[ix], &[&issuer.ct_authority]).unwrap();
    deposit(&mut env, &issuer, &user, 1_000).unwrap();
}

#[test]
fn full_confidential_lifecycle() {
    let (mut env, issuer) = setup();
    let alice = confidential_user(&mut env, &issuer, 100_000);
    let bob = confidential_user(&mut env, &issuer, 0);

    deposit(&mut env, &issuer, &alice, 60_000).unwrap();
    assert_eq!(balance(&env, &alice.acc), 40_000);
    assert_eq!(
        confidential::pending_balance(&env.data(&alice.acc), &alice.keys).unwrap(),
        60_000
    );
    assert_eq!(
        confidential::available_balance(&env.data(&alice.acc), &alice.keys).unwrap(),
        0
    );

    apply_pending(&mut env, &alice);
    assert_eq!(
        confidential::available_balance(&env.data(&alice.acc), &alice.keys).unwrap(),
        60_000
    );

    send_confidential(&mut env, &issuer, &alice, &bob, 20_000).unwrap();
    assert_eq!(
        confidential::available_balance(&env.data(&alice.acc), &alice.keys).unwrap(),
        40_000
    );
    assert_eq!(
        confidential::pending_balance(&env.data(&bob.acc), &bob.keys).unwrap(),
        19_900
    );

    apply_pending(&mut env, &bob);
    withdraw(&mut env, &issuer, &bob, 15_000).unwrap();
    assert_eq!(balance(&env, &bob.acc), 15_000);
    assert_eq!(
        confidential::available_balance(&env.data(&bob.acc), &bob.keys).unwrap(),
        4_900
    );
}

#[test]
fn withdraw_before_applying_pending_fails() {
    let (mut env, issuer) = setup();
    let alice = confidential_user(&mut env, &issuer, 10_000);
    deposit(&mut env, &issuer, &alice, 10_000).unwrap();

    assert!(withdraw(&mut env, &issuer, &alice, 5_000).is_err());

    apply_pending(&mut env, &alice);
    withdraw(&mut env, &issuer, &alice, 5_000).unwrap();
    assert_eq!(balance(&env, &alice.acc), 5_000);
}

#[test]
fn cannot_send_more_than_the_confidential_balance() {
    let (mut env, issuer) = setup();
    let alice = confidential_user(&mut env, &issuer, 10_000);
    let bob = confidential_user(&mut env, &issuer, 0);
    deposit(&mut env, &issuer, &alice, 10_000).unwrap();
    apply_pending(&mut env, &alice);
    assert!(send_confidential(&mut env, &issuer, &alice, &bob, 10_001).is_err());
}

#[test]
fn permanent_delegate_seizes_the_public_balance() {
    let (mut env, issuer) = setup();
    let user = env.wallet();
    let acc = issuer.onboard(&mut env, &user.pubkey(), 50_000);
    let treasury = issuer.onboard(&mut env, &issuer.mint_authority.pubkey(), 0);

    seize(&mut env, &issuer, &acc, &treasury, 50_000).unwrap();
    assert_eq!(balance(&env, &acc), 0);
}

#[test]
fn seizure_misses_funds_moved_into_the_confidential_balance() {
    let (mut env, issuer) = setup();
    let sanctioned = confidential_user(&mut env, &issuer, 50_000);
    let treasury = issuer.onboard(&mut env, &issuer.mint_authority.pubkey(), 0);

    deposit(&mut env, &issuer, &sanctioned, 50_000).unwrap();
    apply_pending(&mut env, &sanctioned);

    assert_eq!(balance(&env, &sanctioned.acc), 0);
    assert!(seize(&mut env, &issuer, &sanctioned.acc, &treasury, 50_000).is_err());
    assert!(seize(&mut env, &issuer, &sanctioned.acc, &treasury, 1).is_err());

    let ix = kyc::freeze(
        &sanctioned.acc,
        &issuer.mint(),
        &issuer.freeze_authority.pubkey(),
    );
    env.send(&[ix], &[&issuer.freeze_authority]).unwrap();
    assert!(withdraw(&mut env, &issuer, &sanctioned, 10_000).is_err());
    assert!(
        read::token_account(&env.data(&sanctioned.acc))
            .unwrap()
            .frozen
    );
}
