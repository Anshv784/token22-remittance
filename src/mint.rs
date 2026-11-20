use solana_instruction::Instruction;
use solana_pubkey::Pubkey;
use solana_system_interface::instruction::create_account;
use solana_zk_sdk::encryption::pod::elgamal::PodElGamalPubkey;
use spl_token_2022_interface::{
    extension::{
        confidential_transfer, confidential_transfer_fee, default_account_state, metadata_pointer,
        transfer_fee, ExtensionType,
    },
    instruction as token_ix,
    state::{AccountState, Mint},
    ID as TOKEN_2022,
};
use spl_token_metadata_interface::state::TokenMetadata;

pub struct MintParams {
    pub decimals: u8,
    pub fee_bps: u16,
    pub max_fee: u64,
    pub mint_authority: Pubkey,
    pub freeze_authority: Pubkey,
    pub fee_config_authority: Pubkey,
    pub fee_withdraw_authority: Pubkey,
    pub close_authority: Pubkey,
    pub name: String,
    pub symbol: String,
    pub uri: String,
    pub v2: Option<V2Params>,
}

pub struct V2Params {
    pub permanent_delegate: Pubkey,
    pub confidential_authority: Pubkey,
    pub auditor_elgamal: Option<PodElGamalPubkey>,
    pub withheld_fee_elgamal: PodElGamalPubkey,
}

impl MintParams {
    pub fn extensions(&self) -> Vec<ExtensionType> {
        let mut ext = vec![
            ExtensionType::TransferFeeConfig,
            ExtensionType::MetadataPointer,
            ExtensionType::DefaultAccountState,
            ExtensionType::MintCloseAuthority,
        ];
        if self.v2.is_some() {
            ext.push(ExtensionType::PermanentDelegate);
            ext.push(ExtensionType::ConfidentialTransferMint);
            ext.push(ExtensionType::ConfidentialTransferFeeConfig);
        }
        ext
    }

    pub fn metadata(&self, mint: &Pubkey) -> TokenMetadata {
        TokenMetadata {
            update_authority: Some(self.mint_authority).try_into().unwrap(),
            mint: *mint,
            name: self.name.clone(),
            symbol: self.symbol.clone(),
            uri: self.uri.clone(),
            additional_metadata: vec![],
        }
    }

    pub fn space(&self) -> usize {
        ExtensionType::try_calculate_account_len::<Mint>(&self.extensions()).unwrap()
    }

    pub fn rent_bytes(&self, mint: &Pubkey) -> usize {
        self.space() + self.metadata(mint).tlv_size_of().unwrap()
    }
}

pub fn create_mint_ixs(
    payer: &Pubkey,
    mint: &Pubkey,
    params: &MintParams,
    lamports: u64,
) -> Vec<Instruction> {
    let mut ixs = vec![create_account(
        payer,
        mint,
        lamports,
        params.space() as u64,
        &TOKEN_2022,
    )];

    ixs.push(
        transfer_fee::instruction::initialize_transfer_fee_config(
            &TOKEN_2022,
            mint,
            Some(&params.fee_config_authority),
            Some(&params.fee_withdraw_authority),
            params.fee_bps,
            params.max_fee,
        )
        .unwrap(),
    );
    ixs.push(
        metadata_pointer::instruction::initialize(
            &TOKEN_2022,
            mint,
            Some(params.mint_authority),
            Some(*mint),
        )
        .unwrap(),
    );
    ixs.push(
        default_account_state::instruction::initialize_default_account_state(
            &TOKEN_2022,
            mint,
            &AccountState::Frozen,
        )
        .unwrap(),
    );
    ixs.push(
        token_ix::initialize_mint_close_authority(&TOKEN_2022, mint, Some(&params.close_authority))
            .unwrap(),
    );

    if let Some(v2) = &params.v2 {
        ixs.push(
            token_ix::initialize_permanent_delegate(&TOKEN_2022, mint, &v2.permanent_delegate)
                .unwrap(),
        );
        ixs.push(
            confidential_transfer::instruction::initialize_mint(
                &TOKEN_2022,
                mint,
                Some(v2.confidential_authority),
                false,
                v2.auditor_elgamal,
            )
            .unwrap(),
        );
        ixs.push(
            confidential_transfer_fee::instruction::initialize_confidential_transfer_fee_config(
                &TOKEN_2022,
                mint,
                Some(params.fee_withdraw_authority),
                &v2.withheld_fee_elgamal,
            )
            .unwrap(),
        );
    }

    ixs.push(
        token_ix::initialize_mint2(
            &TOKEN_2022,
            mint,
            &params.mint_authority,
            Some(&params.freeze_authority),
            params.decimals,
        )
        .unwrap(),
    );

    ixs.push(spl_token_metadata_interface::instruction::initialize(
        &TOKEN_2022,
        mint,
        &params.mint_authority,
        mint,
        &params.mint_authority,
        params.name.clone(),
        params.symbol.clone(),
        params.uri.clone(),
    ));

    ixs
}
