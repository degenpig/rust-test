use borsh::{BorshDeserialize, BorshSchema, BorshSerialize};
use solana_program::borsh::try_from_slice_unchecked;
use solana_program::clock::Clock;
use solana_program::program::{invoke, invoke_signed};
use solana_program::{
    self,
    account_info::{next_account_info, AccountInfo},
    entrypoint,
    entrypoint::ProgramResult,
    program_error::ProgramError,
    pubkey::Pubkey,
    system_instruction,
    sysvar::{rent::Rent, Sysvar},
};
use spl_associated_token_account;
use spl_token;

// Declare and export the program's entrypoint
entrypoint!(process_instruction);

#[derive(Clone, Debug, PartialEq, BorshDeserialize, BorshSerialize, BorshSchema)]
enum MarketplaceInstruction {
    GenerateVault,
    Stake {
        #[allow(dead_code)]
        amount: u64,
    },
    Withdraw {
        #[allow(dead_code)]
        amount: u64,
    },
    Claim,
}

#[derive(Clone, Debug, PartialEq, BorshDeserialize, BorshSerialize, BorshSchema)]
struct StakeData {
    staker: Pubkey,       // 32
    amount: u64,          // 8
    remained_reward: u64, // 8
    last_claim_time: i64, // 8
}

// Program entrypoint's implementation
pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    let accounts_iter = &mut accounts.iter();
    let instruction: MarketplaceInstruction = try_from_slice_unchecked(instruction_data).unwrap();
    const VAULT_PREFIX: &str = "vault";
    const STAKE_PREFIX: &str = "stake";
    const STAKE_PDA_SIZE: u64 = 56;
    const REWARD_GENERATE_RATE: u64 = 250; // 2.5%

    let admin = "5kuLovV9TxV7784KJd97WHhgXTeuX47t6iyuvyqH6BwV"
        .parse::<Pubkey>()
        .unwrap();
    let stake_token_mint = "5kuLovV9TxV7784KJd97WHhgXTeuX47t6iyuvyqH6BwV"
        .parse::<Pubkey>()
        .unwrap();
    let reward_token_mint = "5kuLovV9TxV7784KJd97WHhgXTeuX47t6iyuvyqH6BwV"
        .parse::<Pubkey>()
        .unwrap();

    match instruction {
        MarketplaceInstruction::Stake { amount } => {
            let payer = next_account_info(accounts_iter)?;
            let stake_data_info = next_account_info(accounts_iter)?;
            let mint_info = next_account_info(accounts_iter)?;
            let vault_pda_info = next_account_info(accounts_iter)?;
            let vault_pda_mint_holder_info = next_account_info(accounts_iter)?;
            let vault_mint_holder_info = next_account_info(accounts_iter)?;

            let token_info = next_account_info(accounts_iter)?;
            let assoc_acccount_info = next_account_info(accounts_iter)?;
            let sys_info = next_account_info(accounts_iter)?;
            let rent_info = next_account_info(accounts_iter)?;

            let rent = &Rent::from_account_info(rent_info)?;

            let (data_address, data_address_bump) = Pubkey::find_program_address(
                &[STAKE_PREFIX.as_bytes(), &payer.key.to_bytes()],
                &program_id,
            );

            // program token vault
            let (vault_pda, _) =
                Pubkey::find_program_address(&[&VAULT_PREFIX.as_bytes()], &program_id);

            // stake token vault ata
            let vault_pda_mint_holder = spl_associated_token_account::get_associated_token_address(
                &vault_pda,
                mint_info.key,
            );
            let vault_mint_holder = spl_associated_token_account::get_associated_token_address(
                payer.key,
                mint_info.key,
            );

            if !payer.is_signer {
                // msg!("Unauthorized access");
                return Err(ProgramError::Custom(0x31));
            }
            if *stake_data_info.key != data_address {
                // wrong stake_data_info
                return Err(ProgramError::Custom(0x32));
            }
            if *mint_info.key != stake_token_mint {
                //msg!("Wrong stake token mint");
                return Err(ProgramError::Custom(0x33));
            }
            if vault_pda_mint_holder != *vault_pda_mint_holder_info.key {
                //msg!("Wrong vault_pda_mint_holder");
                return Err(ProgramError::Custom(0x34));
            }
            if vault_mint_holder != *vault_mint_holder_info.key {
                //msg!("Wrong vault_pda_mint_holder");
                return Err(ProgramError::Custom(0x34));
            }

            let timestamp = Clock::get()?.unix_timestamp;

            // initialize stake info PDA if not exist
            // update generated reward and new staking amount if exist
            if stake_data_info.owner != program_id {
                let required_lamports = rent
                    .minimum_balance(STAKE_PDA_SIZE as usize)
                    .max(1)
                    .saturating_sub(stake_data_info.lamports());
                invoke(
                    &system_instruction::transfer(payer.key, &data_address, required_lamports),
                    &[payer.clone(), stake_data_info.clone(), sys_info.clone()],
                )?;
                invoke_signed(
                    &system_instruction::allocate(&data_address, STAKE_PDA_SIZE),
                    &[stake_data_info.clone(), sys_info.clone()],
                    &[&[
                        STAKE_PREFIX.as_bytes(),
                        &payer.key.to_bytes(),
                        &[data_address_bump],
                    ]],
                )?;

                invoke_signed(
                    &system_instruction::assign(&data_address, program_id),
                    &[stake_data_info.clone(), sys_info.clone()],
                    &[&[
                        STAKE_PREFIX.as_bytes(),
                        &payer.key.to_bytes(),
                        &[data_address_bump],
                    ]],
                )?;

                let stake_struct = StakeData {
                    staker: *payer.key,
                    amount,
                    remained_reward: 0,
                    last_claim_time: timestamp,
                };
                stake_struct.serialize(&mut &mut stake_data_info.data.borrow_mut()[..])?;
            } else {
                let mut stake_data =
                    if let Ok(data) = StakeData::try_from_slice(&stake_data_info.data.borrow()) {
                        data
                    } else {
                        // msg!("No stake data account");
                        return Err(ProgramError::Custom(0x35));
                    };

                if *payer.key != stake_data.staker {
                    // mismatched stake pda owner
                    return Err(ProgramError::Custom(0x36));
                }

                let reward = stake_data.amount as u128
                    * (timestamp - stake_data.last_claim_time) as u128
                    * REWARD_GENERATE_RATE as u128
                    / 10000;

                stake_data.amount += amount;
                stake_data.remained_reward = (stake_data.remained_reward as u128 + reward) as u64;
                stake_data.last_claim_time = timestamp;
                stake_data.serialize(&mut &mut stake_data_info.data.borrow_mut()[..])?;
            }

            // create vault ata
            if vault_pda_mint_holder_info.owner != token_info.key {
                invoke(
                    &spl_associated_token_account::create_associated_token_account(
                        payer.key,
                        &vault_pda,
                        mint_info.key,
                    ),
                    &[
                        payer.clone(),
                        vault_pda_mint_holder_info.clone(),
                        vault_pda_info.clone(),
                        mint_info.clone(),
                        sys_info.clone(),
                        token_info.clone(),
                        rent_info.clone(),
                        assoc_acccount_info.clone(),
                    ],
                )?;
            }

            // transfer staking token to vault
            invoke(
                &spl_token::instruction::transfer(
                    token_info.key,
                    vault_mint_holder_info.key,
                    vault_pda_mint_holder_info.key,
                    payer.key,
                    &[],
                    amount,
                )?,
                &[
                    vault_pda_mint_holder_info.clone(),
                    vault_mint_holder_info.clone(),
                    payer.clone(),
                    token_info.clone(),
                ],
            )?;
        }
        MarketplaceInstruction::Withdraw { amount } => {
            let payer = next_account_info(accounts_iter)?;
            let stake_data_info = next_account_info(accounts_iter)?;
            let mint_info = next_account_info(accounts_iter)?;
            let vault_pda_info = next_account_info(accounts_iter)?;
            let vault_pda_mint_holder_info = next_account_info(accounts_iter)?;
            let vault_mint_holder_info = next_account_info(accounts_iter)?;

            let token_info = next_account_info(accounts_iter)?;
            let assoc_acccount_info = next_account_info(accounts_iter)?;
            let sys_info = next_account_info(accounts_iter)?;
            let rent_info = next_account_info(accounts_iter)?;

            let (data_address, _) = Pubkey::find_program_address(
                &[STAKE_PREFIX.as_bytes(), &payer.key.to_bytes()],
                &program_id,
            );

            // program token vault
            let (vault_pda, vault_bump) =
                Pubkey::find_program_address(&[&VAULT_PREFIX.as_bytes()], &program_id);

            // stake token vault ata
            let vault_pda_mint_holder = spl_associated_token_account::get_associated_token_address(
                &vault_pda,
                mint_info.key,
            );
            let vault_mint_holder = spl_associated_token_account::get_associated_token_address(
                payer.key,
                mint_info.key,
            );

            if !payer.is_signer {
                // msg!("Unauthorized access");
                return Err(ProgramError::Custom(0x41));
            }
            if *stake_data_info.key != data_address {
                // wrong stake_data_info
                return Err(ProgramError::Custom(0x42));
            }
            if stake_data_info.owner != program_id {
                // uninitialized stake_data_info
                return Err(ProgramError::Custom(0x43));
            }
            if *mint_info.key != stake_token_mint {
                //msg!("Wrong stake token mint");
                return Err(ProgramError::Custom(0x44));
            }
            if vault_pda_mint_holder != *vault_pda_mint_holder_info.key {
                //msg!("Wrong vault_pda_mint_holder");
                return Err(ProgramError::Custom(0x45));
            }
            if vault_mint_holder != *vault_mint_holder_info.key {
                //msg!("Wrong vault_pda_mint_holder");
                return Err(ProgramError::Custom(0x45));
            }

            let timestamp = Clock::get()?.unix_timestamp;

            let mut stake_data =
                if let Ok(data) = StakeData::try_from_slice(&stake_data_info.data.borrow()) {
                    data
                } else {
                    // msg!("No stake data account");
                    return Err(ProgramError::Custom(0x46));
                };

            if *payer.key != stake_data.staker {
                // mismatched stake pda owner
                return Err(ProgramError::Custom(0x47));
            }
            if amount > stake_data.amount {
                // withdraw amount overflow
                return Err(ProgramError::Custom(0x48));
            }

            let reward = stake_data.amount as u128
                * (timestamp - stake_data.last_claim_time) as u128
                * REWARD_GENERATE_RATE as u128
                / 10000;

            stake_data.amount -= amount;
            stake_data.remained_reward = (stake_data.remained_reward as u128 + reward) as u64;
            stake_data.last_claim_time = timestamp;
            stake_data.serialize(&mut &mut stake_data_info.data.borrow_mut()[..])?;

            // create user ata
            if vault_mint_holder_info.owner != token_info.key {
                invoke(
                    &spl_associated_token_account::create_associated_token_account(
                        payer.key,
                        payer.key,
                        mint_info.key,
                    ),
                    &[
                        payer.clone(),
                        vault_mint_holder_info.clone(),
                        payer.clone(),
                        mint_info.clone(),
                        sys_info.clone(),
                        token_info.clone(),
                        rent_info.clone(),
                        assoc_acccount_info.clone(),
                    ],
                )?;
            }

            invoke_signed(
                &spl_token::instruction::transfer(
                    token_info.key,
                    vault_pda_mint_holder_info.key,
                    vault_mint_holder_info.key,
                    vault_pda_info.key,
                    &[],
                    amount,
                )?,
                &[
                    vault_pda_mint_holder_info.clone(),
                    vault_mint_holder_info.clone(),
                    vault_pda_info.clone(),
                    token_info.clone(),
                ],
                &[&[&VAULT_PREFIX.as_bytes(), &[vault_bump]]],
            )?;
        }
        MarketplaceInstruction::Claim => {
            let payer = next_account_info(accounts_iter)?;
            let stake_data_info = next_account_info(accounts_iter)?;
            let mint_info = next_account_info(accounts_iter)?;
            let vault_pda_info = next_account_info(accounts_iter)?;
            let vault_pda_mint_holder_info = next_account_info(accounts_iter)?;
            let vault_mint_holder_info = next_account_info(accounts_iter)?;

            let token_info = next_account_info(accounts_iter)?;
            let assoc_acccount_info = next_account_info(accounts_iter)?;
            let sys_info = next_account_info(accounts_iter)?;
            let rent_info = next_account_info(accounts_iter)?;

            let (data_address, _) = Pubkey::find_program_address(
                &[STAKE_PREFIX.as_bytes(), &payer.key.to_bytes()],
                &program_id,
            );

            // program token vault
            let (vault_pda, vault_bump) =
                Pubkey::find_program_address(&[&VAULT_PREFIX.as_bytes()], &program_id);

            // stake token vault ata
            let vault_pda_mint_holder = spl_associated_token_account::get_associated_token_address(
                &vault_pda,
                mint_info.key,
            );
            let vault_mint_holder = spl_associated_token_account::get_associated_token_address(
                payer.key,
                mint_info.key,
            );

            if !payer.is_signer {
                // msg!("Unauthorized access");
                return Err(ProgramError::Custom(0x51));
            }
            if *stake_data_info.key != data_address {
                // wrong stake_data_info
                return Err(ProgramError::Custom(0x52));
            }
            if stake_data_info.owner != program_id {
                // uninitialized stake_data_info
                return Err(ProgramError::Custom(0x53));
            }
            if *mint_info.key != reward_token_mint {
                //msg!("Wrong reward token mint");
                return Err(ProgramError::Custom(0x54));
            }
            if vault_pda_mint_holder != *vault_pda_mint_holder_info.key {
                //msg!("Wrong vault_pda_mint_holder");
                return Err(ProgramError::Custom(0x55));
            }
            if vault_pda_mint_holder_info.owner != token_info.key {
                //msg!("Reward vault not initialized");
                return Err(ProgramError::Custom(0x55));
            }
            if vault_mint_holder != *vault_mint_holder_info.key {
                //msg!("Wrong vault_pda_mint_holder");
                return Err(ProgramError::Custom(0x56));
            }

            let timestamp = Clock::get()?.unix_timestamp;

            let mut stake_data =
                if let Ok(data) = StakeData::try_from_slice(&stake_data_info.data.borrow()) {
                    data
                } else {
                    // msg!("No stake data account");
                    return Err(ProgramError::Custom(0x57));
                };

            if *payer.key != stake_data.staker {
                // mismatched stake pda owner
                return Err(ProgramError::Custom(0x58));
            }

            let reward = stake_data.amount as u128
                * (timestamp - stake_data.last_claim_time) as u128
                * REWARD_GENERATE_RATE as u128
                / 10000;
            let reward_amount = (stake_data.remained_reward as u128 + reward) as u64;
            stake_data.remained_reward = 0;
            stake_data.last_claim_time = timestamp;
            stake_data.serialize(&mut &mut stake_data_info.data.borrow_mut()[..])?;

            // create user ata
            if vault_mint_holder_info.owner != token_info.key {
                invoke(
                    &spl_associated_token_account::create_associated_token_account(
                        payer.key,
                        payer.key,
                        mint_info.key,
                    ),
                    &[
                        payer.clone(),
                        vault_mint_holder_info.clone(),
                        payer.clone(),
                        mint_info.clone(),
                        sys_info.clone(),
                        token_info.clone(),
                        rent_info.clone(),
                        assoc_acccount_info.clone(),
                    ],
                )?;
            }

            invoke_signed(
                &spl_token::instruction::transfer(
                    token_info.key,
                    vault_pda_mint_holder_info.key,
                    vault_mint_holder_info.key,
                    vault_pda_info.key,
                    &[],
                    reward_amount,
                )?,
                &[
                    vault_pda_mint_holder_info.clone(),
                    vault_mint_holder_info.clone(),
                    vault_pda_info.clone(),
                    token_info.clone(),
                ],
                &[&[&VAULT_PREFIX.as_bytes(), &[vault_bump]]],
            )?;
        }
        MarketplaceInstruction::GenerateVault => {
            let (vault_pda, vault_bump_seed) =
                Pubkey::find_program_address(&[VAULT_PREFIX.as_bytes()], &program_id);

            let payer = next_account_info(accounts_iter)?;
            let pda = next_account_info(accounts_iter)?;
            let system_program = next_account_info(accounts_iter)?;
            let rent_info = next_account_info(accounts_iter)?;

            let rent = &Rent::from_account_info(rent_info)?;

            if pda.key != &vault_pda {
                //msg!("Wrong account generated by client");
                return Err(ProgramError::Custom(0x00));
            }

            if pda.owner == program_id {
                //msg!("Account already assigned");
                return Err(ProgramError::Custom(0x01));
            }

            if *payer.key != admin || !payer.is_signer {
                //unauthorized access
                return Err(ProgramError::Custom(0x02));
            }
            let required_lamports = rent
                .minimum_balance(0)
                .max(1)
                .saturating_sub(pda.lamports());
            invoke(
                &system_instruction::transfer(payer.key, &vault_pda, required_lamports),
                &[payer.clone(), pda.clone(), system_program.clone()],
            )?;

            invoke_signed(
                &system_instruction::assign(&vault_pda, program_id),
                &[pda.clone(), system_program.clone()],
                &[&[VAULT_PREFIX.as_bytes(), &[vault_bump_seed]]],
            )?;
        }
    };

    Ok(())
}
