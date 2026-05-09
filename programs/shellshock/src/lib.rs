use anchor_lang::prelude::*;

declare_id!("Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS");

#[program]
pub mod shellshock {
    use super::*;

    pub fn create_game(ctx: Context<CreateGame>, bet_lamports: u64) -> Result<()> {
        require!(bet_lamports > 0, ShellshockError::InvalidBet);

        let game = &mut ctx.accounts.game;
        game.player_1 = ctx.accounts.player_1.key();
        game.player_2 = Pubkey::default();
        game.bet_lamports = bet_lamports;
        game.phase = GamePhase::WaitingForPlayer;
        game.turn = 0;
        game.player_1_health = 100;
        game.player_2_health = 100;
        game.chamber_position = 0;
        game.chamber_flags = 0;
        game.bullets_loaded = 0;
        game.actions_this_round = 0;
        game.round = 0;
        game.winner = None;
        game.bump = ctx.bumps.game;

        let vault = &mut ctx.accounts.vault;
        vault.bump = ctx.bumps.vault;

        transfer_lamports(
            &ctx.accounts.player_1,
            &vault.to_account_info(),
            bet_lamports,
            &ctx.accounts.system_program,
        )?;

        emit!(GameCreated {
            game: game.key(),
            player_1: ctx.accounts.player_1.key(),
            bet_lamports,
        });

        Ok(())
    }

    pub fn join_game(ctx: Context<JoinGame>) -> Result<()> {
        let game = &mut ctx.accounts.game;

        require!(
            game.phase == GamePhase::WaitingForPlayer,
            ShellshockError::InvalidPhase
        );
        require!(
            game.player_2 == Pubkey::default(),
            ShellshockError::GameFull
        );
        require!(
            ctx.accounts.player_2.key() != game.player_1,
            ShellshockError::CannotPlayWithSelf
        );

        game.player_2 = ctx.accounts.player_2.key();
        game.phase = GamePhase::WaitingForVrf;

        transfer_lamports(
            &ctx.accounts.player_2,
            &ctx.accounts.vault.to_account_info(),
            game.bet_lamports,
            &ctx.accounts.system_program,
        )?;

        emit!(PlayerJoined {
            game: game.key(),
            player_2: ctx.accounts.player_2.key(),
        });

        Ok(())
    }

    pub fn initialize_round(ctx: Context<InitializeRound>, seed: u64) -> Result<()> {
        let game = &mut ctx.accounts.game;

        require!(
            game.phase == GamePhase::WaitingForVrf,
            ShellshockError::InvalidPhase
        );
        require!(
            game.player_2 != Pubkey::default(),
            ShellshockError::GameNotFull
        );

        let clock = Clock::get()?;
        let entropy = (clock.slot)
            .wrapping_mul(seed)
            .wrapping_add(clock.unix_timestamp as u64);

        let mut flags: u8 = 0;
        let mut rng = entropy;
        let mut count = 0;
        while count < 3 {
            let pos = (rng % 6) as u8;
            if flags & (1 << pos) == 0 {
                flags |= 1 << pos;
                count += 1;
            }
            rng = rng.wrapping_mul(1103515245).wrapping_add(12345);
        }

        game.chamber_flags = flags;
        game.bullets_loaded = 3;
        game.chamber_position = 0;
        game.actions_this_round = 0;
        game.turn = 0;
        game.phase = GamePhase::Playing;
        game.round = game.round.checked_add(1).unwrap();

        emit!(RoundStarted {
            game: game.key(),
            round: game.round,
            chamber_flags: flags,
        });

        Ok(())
    }

    pub fn shoot(ctx: Context<PlayerAction>) -> Result<()> {
        let game = &mut ctx.accounts.game;
        let player = ctx.accounts.player.key();

        require!(
            game.phase == GamePhase::Playing,
            ShellshockError::InvalidPhase
        );
        require!(game.is_player_turn(&player), ShellshockError::NotYourTurn);

        let bullet_present = (game.chamber_flags >> game.chamber_position) & 1 == 1;

        if bullet_present {
            game.chamber_flags &= !(1 << game.chamber_position);
            game.bullets_loaded = game.bullets_loaded.saturating_sub(1);

            if game.turn == 0 {
                game.player_2_health = game.player_2_health.saturating_sub(30);
            } else {
                game.player_1_health = game.player_1_health.saturating_sub(30);
            }
        }

        advance_and_resolve_round(game, player, bullet_present)?;

        emit!(ShotFired {
            game: game.key(),
            shooter: player,
            bullet: bullet_present,
            chamber: game.chamber_position,
        });

        Ok(())
    }

    pub fn reload(ctx: Context<PlayerAction>) -> Result<()> {
        let game = &mut ctx.accounts.game;
        let player = ctx.accounts.player.key();

        require!(
            game.phase == GamePhase::Playing,
            ShellshockError::InvalidPhase
        );
        require!(game.is_player_turn(&player), ShellshockError::NotYourTurn);
        require!(game.bullets_loaded < 6, ShellshockError::ChamberFull);

        if (game.chamber_flags >> game.chamber_position) & 1 == 0 {
            game.chamber_flags |= 1 << game.chamber_position;
            game.bullets_loaded = game.bullets_loaded.checked_add(1).unwrap();
        }

        advance_and_resolve_round(game, player, false)?;

        emit!(GunReloaded {
            game: game.key(),
            player,
            chamber: game.chamber_position,
        });

        Ok(())
    }

    pub fn use_item(ctx: Context<PlayerAction>, item_type: u8) -> Result<()> {
        let game = &mut ctx.accounts.game;
        let player = ctx.accounts.player.key();

        require!(
            game.phase == GamePhase::Playing,
            ShellshockError::InvalidPhase
        );
        require!(game.is_player_turn(&player), ShellshockError::NotYourTurn);

        match item_type {
            1 => {
                let target = if game.turn == 0 {
                    &mut game.player_1_health
                } else {
                    &mut game.player_2_health
                };
                *target = target.saturating_add(20).min(100);
            }
            _ => return Err(ShellshockError::InvalidItem.into()),
        }

        advance_and_resolve_round(game, player, false)?;

        emit!(ItemUsed {
            game: game.key(),
            player,
            item_type,
        });

        Ok(())
    }

    pub fn claim_reward(ctx: Context<ClaimReward>) -> Result<()> {
        let game = &ctx.accounts.game;

        require!(
            game.phase == GamePhase::Finished,
            ShellshockError::InvalidPhase
        );
        require!(
            Some(ctx.accounts.winner.key()) == game.winner,
            ShellshockError::NotWinner
        );

        emit!(RewardClaimed {
            game: game.key(),
            winner: ctx.accounts.winner.key(),
            amount: ctx.accounts.vault.get_lamports(),
        });

        Ok(())
    }
}

fn transfer_lamports<'info>(
    from: &Signer<'info>,
    to: &AccountInfo<'info>,
    amount: u64,
    system_program: &Program<'info, System>,
) -> Result<()> {
    let ix = anchor_lang::solana_program::system_instruction::transfer(
        &from.key(),
        &to.key(),
        amount,
    );
    anchor_lang::solana_program::program::invoke(
        &ix,
        &[
            from.to_account_info(),
            to.clone(),
            system_program.to_account_info(),
        ],
    )?;
    Ok(())
}

fn advance_and_resolve_round(game: &mut Account<Game>, _player: Pubkey, _bullet: bool) -> Result<()> {
    game.actions_this_round = game.actions_this_round.checked_add(1).unwrap();
    game.chamber_position = (game.chamber_position + 1) % 6;

    if game.player_1_health == 0 || game.player_2_health == 0 {
        game.phase = GamePhase::Finished;
        game.winner = Some(if game.player_1_health > 0 {
            game.player_1
        } else {
            game.player_2
        });
        emit!(GameFinished {
            game: game.key(),
            winner: game.winner.unwrap(),
        });
    } else if game.bullets_loaded == 0 || game.actions_this_round >= 6 {
        game.phase = GamePhase::WaitingForVrf;
        emit!(RoundEnded {
            game: game.key(),
            round: game.round,
        });
    } else {
        game.turn = if game.turn == 0 { 1 } else { 0 };
    }

    Ok(())
}

#[account]
#[derive(InitSpace)]
pub struct Game {
    pub player_1: Pubkey,
    pub player_2: Pubkey,
    pub bet_lamports: u64,
    pub phase: GamePhase,
    pub turn: u8,
    pub player_1_health: u8,
    pub player_2_health: u8,
    pub chamber_position: u8,
    pub chamber_flags: u8,
    pub bullets_loaded: u8,
    pub actions_this_round: u8,
    pub round: u8,
    pub winner: Option<Pubkey>,
    pub bump: u8,
}

#[account]
#[derive(InitSpace)]
pub struct Vault {
    pub bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, InitSpace)]
pub enum GamePhase {
    WaitingForPlayer,
    WaitingForVrf,
    Playing,
    Finished,
}

#[error_code]
pub enum ShellshockError {
    #[msg("Bet amount must be greater than 0")]
    InvalidBet,
    #[msg("Invalid game phase for this action")]
    InvalidPhase,
    #[msg("Game is already full")]
    GameFull,
    #[msg("Cannot play against yourself")]
    CannotPlayWithSelf,
    #[msg("Game is not full yet")]
    GameNotFull,
    #[msg("Not your turn")]
    NotYourTurn,
    #[msg("Chamber is at maximum capacity")]
    ChamberFull,
    #[msg("Only the winner can claim the reward")]
    NotWinner,
    #[msg("Invalid item type")]
    InvalidItem,
}

#[derive(Accounts)]
#[instruction(bet_lamports: u64)]
pub struct CreateGame<'info> {
    #[account(
        init,
        payer = player_1,
        space = 8 + Game::INIT_SPACE,
        seeds = [b"game", player_1.key().as_ref(), &bet_lamports.to_le_bytes()],
        bump,
    )]
    pub game: Account<'info, Game>,
    #[account(
        init,
        payer = player_1,
        space = 8 + Vault::INIT_SPACE,
        seeds = [b"vault", game.key().as_ref()],
        bump,
    )]
    pub vault: Account<'info, Vault>,
    #[account(mut)]
    pub player_1: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct JoinGame<'info> {
    #[account(
        mut,
        seeds = [b"game", game.player_1.as_ref(), &game.bet_lamports.to_le_bytes()],
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,
    #[account(
        mut,
        seeds = [b"vault", game.key().as_ref()],
        bump = vault.bump,
    )]
    pub vault: Account<'info, Vault>,
    #[account(mut)]
    pub player_2: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct InitializeRound<'info> {
    #[account(
        mut,
        seeds = [b"game", game.player_1.as_ref(), &game.bet_lamports.to_le_bytes()],
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,
}

#[derive(Accounts)]
pub struct PlayerAction<'info> {
    #[account(
        mut,
        seeds = [b"game", game.player_1.as_ref(), &game.bet_lamports.to_le_bytes()],
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,
    pub player: Signer<'info>,
}

#[derive(Accounts)]
pub struct ClaimReward<'info> {
    #[account(
        mut,
        seeds = [b"game", game.player_1.as_ref(), &game.bet_lamports.to_le_bytes()],
        bump = game.bump,
        close = winner,
    )]
    pub game: Account<'info, Game>,
    #[account(
        mut,
        seeds = [b"vault", game.key().as_ref()],
        bump = vault.bump,
        close = winner,
    )]
    pub vault: Account<'info, Vault>,
    #[account(mut)]
    pub winner: Signer<'info>,
}

#[event]
pub struct GameCreated {
    pub game: Pubkey,
    pub player_1: Pubkey,
    pub bet_lamports: u64,
}

#[event]
pub struct PlayerJoined {
    pub game: Pubkey,
    pub player_2: Pubkey,
}

#[event]
pub struct RoundStarted {
    pub game: Pubkey,
    pub round: u8,
    pub chamber_flags: u8,
}

#[event]
pub struct ShotFired {
    pub game: Pubkey,
    pub shooter: Pubkey,
    pub bullet: bool,
    pub chamber: u8,
}

#[event]
pub struct GunReloaded {
    pub game: Pubkey,
    pub player: Pubkey,
    pub chamber: u8,
}

#[event]
pub struct ItemUsed {
    pub game: Pubkey,
    pub player: Pubkey,
    pub item_type: u8,
}

#[event]
pub struct RoundEnded {
    pub game: Pubkey,
    pub round: u8,
}

#[event]
pub struct GameFinished {
    pub game: Pubkey,
    pub winner: Pubkey,
}

#[event]
pub struct RewardClaimed {
    pub game: Pubkey,
    pub winner: Pubkey,
    pub amount: u64,
}

impl Game {
    pub fn is_player_turn(&self, player: &Pubkey) -> bool {
        let expected = if self.turn == 0 {
            &self.player_1
        } else {
            &self.player_2
        };
        player == expected
    }
}
