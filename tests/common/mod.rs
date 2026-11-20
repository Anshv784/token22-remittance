#![allow(dead_code)]

use litesvm::LiteSVM;
use remit::{
    mint::{create_mint_ixs, MintParams, V2Params},
    TOKEN_2022,
};
use solana_clock::Clock;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_zk_sdk::encryption::elgamal::ElGamalKeypair;
use spl_associated_token_account_interface::{
    address::get_associated_token_address_with_program_id,
    instruction::create_associated_token_account_idempotent,
};
use spl_token_2022_interface::instruction::mint_to_checked;

pub const DECIMALS: u8 = 2;
pub const FEE_BPS: u16 = 50;
pub const MAX_FEE: u64 = 5_000;

pub struct Env {
    pub svm: LiteSVM,
    pub payer: Keypair,
}

impl Env {
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program(
            TOKEN_2022,
            include_bytes!("../fixtures/spl_token_2022_zk.so"),
        )
        .unwrap();
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
        Self { svm, payer }
    }

    pub fn wallet(&mut self) -> Keypair {
        let k = Keypair::new();
        self.svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        k
    }

    pub fn send(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<(), String> {
        let mut all = vec![&self.payer];
        all.extend_from_slice(signers);
        let tx = Transaction::new_signed_with_payer(
            ixs,
            Some(&self.payer.pubkey()),
            &all,
            self.svm.latest_blockhash(),
        );
        let res = self
            .svm
            .send_transaction(tx)
            .map(|_| ())
            .map_err(|e| format!("{:?}", e.err));
        self.svm.expire_blockhash();
        res
    }

    pub fn send_heavy(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<(), String> {
        let mut all = vec![ComputeBudgetInstruction::set_compute_unit_limit(1_400_000)];
        all.extend_from_slice(ixs);
        self.send(&all, signers)
    }

    pub fn data(&self, key: &Pubkey) -> Vec<u8> {
        self.svm
            .get_account(key)
            .map(|a| a.data)
            .unwrap_or_default()
    }

    pub fn exists(&self, key: &Pubkey) -> bool {
        self.svm.get_account(key).is_some_and(|a| a.lamports > 0)
    }

    pub fn epoch(&self) -> u64 {
        self.svm.get_sysvar::<Clock>().epoch
    }

    pub fn set_epoch(&mut self, epoch: u64) {
        let mut clock = self.svm.get_sysvar::<Clock>();
        clock.epoch = epoch;
        self.svm.set_sysvar(&clock);
    }

    pub fn rent(&self, len: usize) -> u64 {
        self.svm.minimum_balance_for_rent_exemption(len)
    }
}

pub struct Issuer {
    pub mint: Keypair,
    pub mint_authority: Keypair,
    pub freeze_authority: Keypair,
    pub fee_authority: Keypair,
    pub close_authority: Keypair,
    pub seizer: Keypair,
    pub ct_authority: Keypair,
    pub withheld_elgamal: ElGamalKeypair,
}

impl Issuer {
    pub fn new() -> Self {
        Self {
            mint: Keypair::new(),
            mint_authority: Keypair::new(),
            freeze_authority: Keypair::new(),
            fee_authority: Keypair::new(),
            close_authority: Keypair::new(),
            seizer: Keypair::new(),
            ct_authority: Keypair::new(),
            withheld_elgamal: ElGamalKeypair::new_rand(),
        }
    }

    pub fn mint(&self) -> Pubkey {
        self.mint.pubkey()
    }

    pub fn params(&self, reissue: bool) -> MintParams {
        MintParams {
            decimals: DECIMALS,
            fee_bps: FEE_BPS,
            max_fee: MAX_FEE,
            mint_authority: self.mint_authority.pubkey(),
            freeze_authority: self.freeze_authority.pubkey(),
            fee_config_authority: self.fee_authority.pubkey(),
            fee_withdraw_authority: self.fee_authority.pubkey(),
            close_authority: self.close_authority.pubkey(),
            name: "Remit Dollar".into(),
            symbol: "RUSD".into(),
            uri: "https://example.com/rusd.json".into(),
            v2: reissue.then(|| V2Params {
                permanent_delegate: self.seizer.pubkey(),
                confidential_authority: self.ct_authority.pubkey(),
                auditor_elgamal: None,
                withheld_fee_elgamal: (*self.withheld_elgamal.pubkey()).into(),
            }),
        }
    }

    pub fn create(&self, env: &mut Env, reissue: bool) {
        let params = self.params(reissue);
        let lamports = env.rent(params.rent_bytes(&self.mint()));
        let ixs = create_mint_ixs(&env.payer.pubkey(), &self.mint(), &params, lamports);
        env.send(&ixs, &[&self.mint, &self.mint_authority]).unwrap();
    }

    pub fn ata(&self, owner: &Pubkey) -> Pubkey {
        get_associated_token_address_with_program_id(owner, &self.mint(), &TOKEN_2022)
    }

    pub fn open_account(&self, env: &mut Env, owner: &Pubkey) -> Pubkey {
        let ix = create_associated_token_account_idempotent(
            &env.payer.pubkey(),
            owner,
            &self.mint(),
            &TOKEN_2022,
        );
        env.send(&[ix], &[]).unwrap();
        self.ata(owner)
    }

    pub fn kyc(&self, env: &mut Env, account: &Pubkey) {
        let ix = remit::kyc::clear_kyc(account, &self.mint(), &self.freeze_authority.pubkey());
        env.send(&[ix], &[&self.freeze_authority]).unwrap();
    }

    pub fn mint_to(&self, env: &mut Env, account: &Pubkey, amount: u64) {
        let ix = mint_to_checked(
            &TOKEN_2022,
            &self.mint(),
            account,
            &self.mint_authority.pubkey(),
            &[],
            amount,
            DECIMALS,
        )
        .unwrap();
        env.send(&[ix], &[&self.mint_authority]).unwrap();
    }

    pub fn onboard(&self, env: &mut Env, owner: &Pubkey, amount: u64) -> Pubkey {
        let acc = self.open_account(env, owner);
        self.kyc(env, &acc);
        if amount > 0 {
            self.mint_to(env, &acc, amount);
        }
        acc
    }
}

pub fn balance(env: &Env, account: &Pubkey) -> u64 {
    remit::read::token_account(&env.data(account))
        .unwrap()
        .amount
}
