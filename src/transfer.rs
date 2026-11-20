use solana_instruction::Instruction;
use solana_program_error::ProgramError;
use solana_pubkey::Pubkey;
use spl_token_2022_interface::{extension::transfer_fee, ID as TOKEN_2022};

use crate::read;

pub fn expected_fee(mint_data: &[u8], epoch: u64, amount: u64) -> Result<u64, ProgramError> {
    read::transfer_fee_config(mint_data)?
        .calculate_epoch_fee(epoch, amount)
        .ok_or(ProgramError::ArithmeticOverflow)
}

#[allow(clippy::too_many_arguments)]
pub fn transfer_ix(
    mint_data: &[u8],
    epoch: u64,
    mint: &Pubkey,
    from: &Pubkey,
    to: &Pubkey,
    authority: &Pubkey,
    amount: u64,
) -> Result<Instruction, ProgramError> {
    let decimals = read::mint_decimals(mint_data)?;
    let fee = expected_fee(mint_data, epoch, amount)?;
    transfer_fee::instruction::transfer_checked_with_fee(
        &TOKEN_2022,
        from,
        mint,
        to,
        authority,
        &[],
        amount,
        decimals,
        fee,
    )
}
