use solana_program_error::ProgramError;
use solana_pubkey::Pubkey;
use spl_token_2022_interface::{
    extension::{
        confidential_transfer::ConfidentialTransferAccount, transfer_fee::TransferFeeConfig,
        BaseStateWithExtensions, StateWithExtensions,
    },
    state::{Account, AccountState, Mint},
};

pub struct TokenAccountView {
    pub owner: Pubkey,
    pub amount: u64,
    pub frozen: bool,
}

pub fn token_account(data: &[u8]) -> Result<TokenAccountView, ProgramError> {
    let acc = StateWithExtensions::<Account>::unpack(data)?;
    Ok(TokenAccountView {
        owner: acc.base.owner,
        amount: acc.base.amount,
        frozen: acc.base.state == AccountState::Frozen,
    })
}

pub fn mint_decimals(data: &[u8]) -> Result<u8, ProgramError> {
    Ok(StateWithExtensions::<Mint>::unpack(data)?.base.decimals)
}

pub fn transfer_fee_config(data: &[u8]) -> Result<TransferFeeConfig, ProgramError> {
    let mint = StateWithExtensions::<Mint>::unpack(data)?;
    Ok(*mint.get_extension::<TransferFeeConfig>()?)
}

pub fn confidential_account(data: &[u8]) -> Result<ConfidentialTransferAccount, ProgramError> {
    let acc = StateWithExtensions::<Account>::unpack(data)?;
    Ok(*acc.get_extension::<ConfidentialTransferAccount>()?)
}
