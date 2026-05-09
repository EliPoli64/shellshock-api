import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, Keypair, SystemProgram, LAMPORTS_PER_SOL } from "@solana/web3.js";
import { expect } from "chai";
import { Shellshock } from "../target/types/shellshock";

describe("shellshock", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.Shellshock as Program<Shellshock>;
  const player1 = provider.wallet;

  let player2: Keypair;
  let gamePda: PublicKey;
  let vaultPda: PublicKey;
  const betLamports = new anchor.BN(1_000_000_000);

  const findPdas = () => {
    const [g] = PublicKey.findProgramAddressSync(
      [
        Buffer.from("game"),
        player1.publicKey.toBuffer(),
        betLamports.toArrayLike(Buffer, "le", 8),
      ],
      program.programId
    );
    gamePda = g;

    const [v] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), gamePda.toBuffer()],
      program.programId
    );
    vaultPda = v;
  };

  before(async () => {
    player2 = Keypair.generate();
    const sig = await provider.connection.requestAirdrop(
      player2.publicKey,
      10 * LAMPORTS_PER_SOL
    );
    await provider.connection.confirmTransaction(sig);
    findPdas();
  });

  it("Creates a game", async () => {
    await program.methods
      .createGame(betLamports)
      .accountsStrict({
        game: gamePda,
        vault: vaultPda,
        player1: player1.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .rpc();

    const game = await program.account.game.fetch(gamePda);
    expect(game.player1.toString()).to.equal(player1.publicKey.toString());
    expect(game.player2.toString()).to.equal(PublicKey.default.toString());
    expect(game.betLamports.eq(betLamports)).to.be.true;
    expect(game.phase).to.have.property("waitingForPlayer");
    expect(game.player1Health).to.equal(100);
    expect(game.player2Health).to.equal(100);
  });

  it("Player 2 joins the game", async () => {
    await program.methods
      .joinGame()
      .accountsStrict({
        game: gamePda,
        vault: vaultPda,
        player2: player2.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([player2])
      .rpc();

    const game = await program.account.game.fetch(gamePda);
    expect(game.player2.toString()).to.equal(player2.publicKey.toString());
    expect(game.phase).to.have.property("waitingForVrf");
  });

  it("Initializes a round with randomness", async () => {
    const seed = new anchor.BN(Math.floor(Math.random() * Number.MAX_SAFE_INTEGER));

    await program.methods
      .initializeRound(seed)
      .accountsStrict({
        game: gamePda,
      })
      .rpc();

    const game = await program.account.game.fetch(gamePda);
    expect(game.phase).to.have.property("playing");
    expect(game.round).to.equal(1);
    expect(game.turn).to.equal(0);
    expect(game.bulletsLoaded).to.equal(3);
    expect(game.actionsThisRound).to.equal(0);
    expect(game.chamberFlags).to.be.greaterThan(0);
  });

  it("Player 1 shoots", async () => {
    const before = await program.account.game.fetch(gamePda);
    const p2HealthBefore = before.player2Health;

    await program.methods
      .shoot()
      .accountsStrict({
        game: gamePda,
        player: player1.publicKey,
      })
      .rpc();

    const after = await program.account.game.fetch(gamePda);
    expect(after.actionsThisRound).to.equal(1);

    // If bullet was present, p2 lost health
    const bulletHit = after.player2Health < p2HealthBefore;
    if (bulletHit) {
      expect(after.player2Health).to.equal(p2HealthBefore - 30);
    }

    // Turn should have switched to player 2 (unless round/game ended)
    if (after.phase.playing) {
      expect(after.turn).to.equal(1);
    }
  });

  it("Player 2 reloads", async () => {
    const game = await program.account.game.fetch(gamePda);
    if (!game.phase.playing) return; // skip if round ended

    await program.methods
      .reload()
      .accountsStrict({
        game: gamePda,
        player: player2.publicKey,
      })
      .signers([player2])
      .rpc();

    const after = await program.account.game.fetch(gamePda);
    if (after.phase.playing) {
      expect(after.turn).to.equal(0);
    }
  });

  it("Player 2 cannot act on player 1's turn", async () => {
    const game = await program.account.game.fetch(gamePda);
    if (!game.phase.playing) return;

    try {
      await program.methods
        .shoot()
        .accountsStrict({
          game: gamePda,
          player: player2.publicKey,
        })
        .signers([player2])
        .rpc();
      expect.fail("Should have thrown");
    } catch (err: any) {
      expect(err.message).to.include("Not your turn");
    }
  });

  it("Creates and completes a full game", async () => {
    const p1 = Keypair.generate();
    const p2 = Keypair.generate();

    await provider.connection.requestAirdrop(p1.publicKey, 10 * LAMPORTS_PER_SOL);
    await provider.connection.requestAirdrop(p2.publicKey, 10 * LAMPORTS_PER_SOL);
    await Promise.all([
      provider.connection.confirmTransaction(
        await provider.connection.requestAirdrop(p1.publicKey, 10 * LAMPORTS_PER_SOL)
      ),
      provider.connection.confirmTransaction(
        await provider.connection.requestAirdrop(p2.publicKey, 10 * LAMPORTS_PER_SOL)
      ),
    ]);

    const bet = new anchor.BN(500_000_000);
    const [gPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("game"), p1.publicKey.toBuffer(), bet.toArrayLike(Buffer, "le", 8)],
      program.programId
    );
    const [vPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), gPda.toBuffer()],
      program.programId
    );

    await program.methods
      .createGame(bet)
      .accountsStrict({
        game: gPda,
        vault: vPda,
        player1: p1.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([p1])
      .rpc();

    await program.methods
      .joinGame()
      .accountsStrict({
        game: gPda,
        vault: vPda,
        player2: p2.publicKey,
        systemProgram: SystemProgram.programId,
      })
      .signers([p2])
      .rpc();

    await program.methods
      .initializeRound(new anchor.BN(42))
      .accountsStrict({ game: gPda })
      .rpc();

    // Play until finished
    let currentGame;
    let attempts = 0;
    while (attempts < 20) {
      currentGame = await program.account.game.fetch(gPda);
      if (currentGame.phase.finished) break;

      const activePlayer = currentGame.turn === 0 ? p1 : p2;
      try {
        await program.methods
          .shoot()
          .accountsStrict({
            game: gPda,
            player: activePlayer.publicKey,
          })
          .signers([activePlayer])
          .rpc();
      } catch {
        await program.methods
          .reload()
          .accountsStrict({
            game: gPda,
            player: activePlayer.publicKey,
          })
          .signers([activePlayer])
          .rpc();
      }
      attempts++;
    }

    expect(currentGame!.phase.finished).to.be.true;
    expect(currentGame!.winner).to.not.be.null;

    // Claim reward
    const winnerKey = currentGame!.winner as PublicKey;
    const winner = winnerKey.equals(p1.publicKey) ? p1 : p2;

    const vaultBalBefore = await provider.connection.getBalance(vPda);

    await program.methods
      .claimReward()
      .accountsStrict({
        game: gPda,
        vault: vPda,
        winner: winner.publicKey,
      })
      .signers([winner])
      .rpc();

    // Vault should be closed (0 lamports, no data)
    const vaultAfter = await provider.connection.getAccountInfo(vPda);
    expect(vaultAfter).to.be.null;
  });

  it("Rejects zero bet", async () => {
    const p = Keypair.generate();
    const zero = new anchor.BN(0);
    const [gPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("game"), p.publicKey.toBuffer(), zero.toArrayLike(Buffer, "le", 8)],
      program.programId
    );
    const [vPda] = PublicKey.findProgramAddressSync(
      [Buffer.from("vault"), gPda.toBuffer()],
      program.programId
    );

    try {
      await program.methods
        .createGame(zero)
        .accountsStrict({
          game: gPda,
          vault: vPda,
          player1: p.publicKey,
          systemProgram: SystemProgram.programId,
        })
        .signers([p])
        .rpc();
      expect.fail("Should have thrown");
    } catch (err: any) {
      expect(err.message).to.include("Bet amount must be greater than 0");
    }
  });
});
