use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use spl_token_2022_interface::{instruction, ID as TOKEN_2022};

pub fn clear_kyc(account: &Pubkey, mint: &Pubkey, freeze_authority: &Pubkey) -> Instruction {
    instruction::thaw_account(&TOKEN_2022, account, mint, freeze_authority, &[]).unwrap()
}

pub fn freeze(account: &Pubkey, mint: &Pubkey, freeze_authority: &Pubkey) -> Instruction {
    instruction::freeze_account(&TOKEN_2022, account, mint, freeze_authority, &[]).unwrap()
}
