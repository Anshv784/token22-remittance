use std::mem::size_of;

use bytemuck::Pod;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_program_error::ProgramError;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_system_interface::instruction::create_account;
use solana_zk_sdk::{
    encryption::{
        auth_encryption::{AeCiphertext, AeKey},
        elgamal::{ElGamalCiphertext, ElGamalKeypair, ElGamalPubkey},
    },
    zk_elgamal_proof_program::{
        self,
        instruction::{close_context_state, ContextStateInfo, ProofInstruction},
        proof_data::{PubkeyValidityProofData, ZkProofData},
        state::ProofContextState,
    },
};
use spl_token_2022_interface::{
    extension::{
        confidential_transfer::{
            instruction as ct_ix, ConfidentialTransferMint, PENDING_BALANCE_LO_BIT_LENGTH,
        },
        confidential_transfer_fee::ConfidentialTransferFeeConfig,
        BaseStateWithExtensions, ExtensionType, StateWithExtensions,
    },
    instruction as token_ix,
    state::Mint,
    ID as TOKEN_2022,
};
use spl_token_confidential_transfer_proof_extraction::instruction::ProofLocation;
use spl_token_confidential_transfer_proof_generation::{
    transfer_with_fee::transfer_with_fee_split_proof_data, withdraw::withdraw_proof_data,
};

use crate::read;

pub struct UserKeys {
    pub elgamal: ElGamalKeypair,
    pub aes: AeKey,
}

impl UserKeys {
    pub fn new_rand() -> Self {
        Self {
            elgamal: ElGamalKeypair::new_rand(),
            aes: AeKey::new_rand(),
        }
    }
}

fn bad_data<E>(_: E) -> ProgramError {
    ProgramError::InvalidAccountData
}

pub fn configure_ixs(
    payer: &Pubkey,
    owner: &Pubkey,
    account: &Pubkey,
    mint: &Pubkey,
    keys: &UserKeys,
) -> Vec<Instruction> {
    let mut ixs = vec![token_ix::reallocate(
        &TOKEN_2022,
        account,
        payer,
        owner,
        &[],
        &[
            ExtensionType::ConfidentialTransferAccount,
            ExtensionType::ConfidentialTransferFeeAmount,
        ],
    )
    .unwrap()];

    let proof = PubkeyValidityProofData::new(&keys.elgamal).unwrap();
    ixs.extend(
        ct_ix::configure_account(
            &TOKEN_2022,
            account,
            mint,
            &keys.aes.encrypt(0).into(),
            65_536,
            owner,
            &[],
            ProofLocation::InstructionOffset(1.try_into().unwrap(), &proof),
        )
        .unwrap(),
    );
    ixs
}

pub fn approve_ix(account: &Pubkey, mint: &Pubkey, ct_authority: &Pubkey) -> Instruction {
    ct_ix::approve_account(&TOKEN_2022, account, mint, ct_authority, &[]).unwrap()
}

pub fn deposit_ix(
    account: &Pubkey,
    mint: &Pubkey,
    owner: &Pubkey,
    amount: u64,
    decimals: u8,
) -> Instruction {
    ct_ix::deposit(&TOKEN_2022, account, mint, amount, decimals, owner, &[]).unwrap()
}

pub fn available_balance(account_data: &[u8], keys: &UserKeys) -> Result<u64, ProgramError> {
    let ct = read::confidential_account(account_data)?;
    let enc: AeCiphertext = ct
        .decryptable_available_balance
        .try_into()
        .map_err(bad_data)?;
    keys.aes
        .decrypt(&enc)
        .ok_or(ProgramError::InvalidAccountData)
}

pub fn pending_balance(account_data: &[u8], keys: &UserKeys) -> Result<u64, ProgramError> {
    let ct = read::confidential_account(account_data)?;
    let lo: ElGamalCiphertext = ct.pending_balance_lo.try_into().map_err(bad_data)?;
    let hi: ElGamalCiphertext = ct.pending_balance_hi.try_into().map_err(bad_data)?;
    let lo = keys
        .elgamal
        .secret()
        .decrypt_u32(&lo)
        .ok_or(ProgramError::InvalidAccountData)?;
    let hi = keys
        .elgamal
        .secret()
        .decrypt_u32(&hi)
        .ok_or(ProgramError::InvalidAccountData)?;
    Ok(lo + (hi << PENDING_BALANCE_LO_BIT_LENGTH))
}

pub fn apply_pending_ix(
    account_data: &[u8],
    account: &Pubkey,
    owner: &Pubkey,
    keys: &UserKeys,
) -> Result<Instruction, ProgramError> {
    let ct = read::confidential_account(account_data)?;
    let new_available =
        available_balance(account_data, keys)? + pending_balance(account_data, keys)?;
    ct_ix::apply_pending_balance(
        &TOKEN_2022,
        account,
        u64::from(ct.pending_balance_credit_counter),
        &keys.aes.encrypt(new_available).into(),
        owner,
        &[],
    )
}

pub struct ProofAccount {
    pub keypair: Keypair,
    pub create: Instruction,
    pub verify: Instruction,
    pub close: Instruction,
}

fn proof_account<T: Pod + ZkProofData<U>, U: Pod>(
    payer: &Pubkey,
    owner: &Pubkey,
    proof: &T,
    which: ProofInstruction,
    rent: &dyn Fn(usize) -> u64,
) -> ProofAccount {
    let keypair = Keypair::new();
    let address = keypair.pubkey();
    let len = size_of::<ProofContextState<U>>();
    let info = || ContextStateInfo {
        context_state_account: &address,
        context_state_authority: owner,
    };
    ProofAccount {
        create: create_account(
            payer,
            &address,
            rent(len),
            len as u64,
            &zk_elgamal_proof_program::id(),
        ),
        verify: which.encode_verify_proof(Some(info()), proof),
        close: close_context_state(info(), payer),
        keypair,
    }
}

pub struct ProofFlow {
    pub proofs: Vec<ProofAccount>,
    pub token_ixs: Vec<Instruction>,
}

#[allow(clippy::too_many_arguments)]
pub fn transfer_flow(
    payer: &Pubkey,
    owner: &Pubkey,
    mint: &Pubkey,
    mint_data: &[u8],
    epoch: u64,
    source: &Pubkey,
    source_data: &[u8],
    destination: &Pubkey,
    destination_data: &[u8],
    keys: &UserKeys,
    amount: u64,
    rent: &dyn Fn(usize) -> u64,
) -> Result<ProofFlow, ProgramError> {
    let src = read::confidential_account(source_data)?;
    let dest_pubkey: ElGamalPubkey = read::confidential_account(destination_data)?
        .elgamal_pubkey
        .try_into()
        .map_err(bad_data)?;

    let mint_state = StateWithExtensions::<Mint>::unpack(mint_data)?;
    let auditor: Option<ElGamalPubkey> = Option::from(
        mint_state
            .get_extension::<ConfidentialTransferMint>()?
            .auditor_elgamal_pubkey,
    )
    .map(|p: solana_zk_sdk::encryption::pod::elgamal::PodElGamalPubkey| p.try_into())
    .transpose()
    .map_err(bad_data)?;
    let withheld_pubkey: ElGamalPubkey = mint_state
        .get_extension::<ConfidentialTransferFeeConfig>()?
        .withdraw_withheld_authority_elgamal_pubkey
        .try_into()
        .map_err(bad_data)?;
    let fee = *read::transfer_fee_config(mint_data)?.get_epoch_fee(epoch);

    let current: ElGamalCiphertext = src.available_balance.try_into().map_err(bad_data)?;
    let current_decryptable: AeCiphertext = src
        .decryptable_available_balance
        .try_into()
        .map_err(bad_data)?;
    let balance = keys
        .aes
        .decrypt(&current_decryptable)
        .ok_or(ProgramError::InvalidAccountData)?;
    let remaining = balance
        .checked_sub(amount)
        .ok_or(ProgramError::InsufficientFunds)?;

    let data = transfer_with_fee_split_proof_data(
        &current,
        &current_decryptable,
        amount,
        &keys.elgamal,
        &keys.aes,
        &dest_pubkey,
        auditor.as_ref(),
        &withheld_pubkey,
        u16::from(fee.transfer_fee_basis_points),
        u64::from(fee.maximum_fee),
    )
    .map_err(|_| ProgramError::InvalidArgument)?;

    let equality = proof_account(
        payer,
        owner,
        &data.equality_proof_data,
        ProofInstruction::VerifyCiphertextCommitmentEquality,
        rent,
    );
    let validity = proof_account(
        payer,
        owner,
        &data
            .transfer_amount_ciphertext_validity_proof_data_with_ciphertext
            .proof_data,
        ProofInstruction::VerifyBatchedGroupedCiphertext3HandlesValidity,
        rent,
    );
    let fee_sigma = proof_account(
        payer,
        owner,
        &data.percentage_with_cap_proof_data,
        ProofInstruction::VerifyPercentageWithCap,
        rent,
    );
    let fee_validity = proof_account(
        payer,
        owner,
        &data.fee_ciphertext_validity_proof_data,
        ProofInstruction::VerifyBatchedGroupedCiphertext2HandlesValidity,
        rent,
    );
    let range = proof_account(
        payer,
        owner,
        &data.range_proof_data,
        ProofInstruction::VerifyBatchedRangeProofU256,
        rent,
    );

    let token_ixs = ct_ix::transfer_with_fee(
        &TOKEN_2022,
        source,
        mint,
        destination,
        &keys.aes.encrypt(remaining).into(),
        &data
            .transfer_amount_ciphertext_validity_proof_data_with_ciphertext
            .ciphertext_lo,
        &data
            .transfer_amount_ciphertext_validity_proof_data_with_ciphertext
            .ciphertext_hi,
        owner,
        &[],
        ProofLocation::ContextStateAccount(&equality.keypair.pubkey()),
        ProofLocation::ContextStateAccount(&validity.keypair.pubkey()),
        ProofLocation::ContextStateAccount(&fee_sigma.keypair.pubkey()),
        ProofLocation::ContextStateAccount(&fee_validity.keypair.pubkey()),
        ProofLocation::ContextStateAccount(&range.keypair.pubkey()),
    )?;

    Ok(ProofFlow {
        proofs: vec![equality, validity, fee_sigma, fee_validity, range],
        token_ixs,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn withdraw_flow(
    payer: &Pubkey,
    owner: &Pubkey,
    mint: &Pubkey,
    decimals: u8,
    account: &Pubkey,
    account_data: &[u8],
    keys: &UserKeys,
    amount: u64,
    rent: &dyn Fn(usize) -> u64,
) -> Result<ProofFlow, ProgramError> {
    let ct = read::confidential_account(account_data)?;
    let current: ElGamalCiphertext = ct.available_balance.try_into().map_err(bad_data)?;
    let balance = available_balance(account_data, keys)?;
    let remaining = balance
        .checked_sub(amount)
        .ok_or(ProgramError::InsufficientFunds)?;

    let data = withdraw_proof_data(&current, balance, amount, &keys.elgamal)
        .map_err(|_| ProgramError::InvalidArgument)?;
    let equality = proof_account(
        payer,
        owner,
        &data.equality_proof_data,
        ProofInstruction::VerifyCiphertextCommitmentEquality,
        rent,
    );
    let range = proof_account(
        payer,
        owner,
        &data.range_proof_data,
        ProofInstruction::VerifyBatchedRangeProofU64,
        rent,
    );

    let token_ixs = ct_ix::withdraw(
        &TOKEN_2022,
        account,
        mint,
        amount,
        decimals,
        &keys.aes.encrypt(remaining).into(),
        owner,
        &[],
        ProofLocation::ContextStateAccount(&equality.keypair.pubkey()),
        ProofLocation::ContextStateAccount(&range.keypair.pubkey()),
    )?;

    Ok(ProofFlow {
        proofs: vec![equality, range],
        token_ixs,
    })
}
