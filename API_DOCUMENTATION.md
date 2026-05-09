# Shellshock API Documentation

This document provides a comprehensive guide to the backend endpoints, including all request and response structures required for frontend integration.

## Base Configuration
- **Base URL**: `http://localhost:3010` (Configurable)
- **Content-Type**: `application/json`

---

## Endpoints

### 1. Health Check
Checks if the backend is running and returns the current version.
- **URL**: `/health`
- **Method**: `GET`
- **Response**:
  ```json
  {
    "status": "ok",
    "version": "0.1.0"
  }
  ```

### 2. Start PvE Match
Initializes a new match against the AI Dealer.
- **URL**: `/match/pve/start`
- **Method**: `POST`
- **Request Body**:
  ```json
  {
    "wallet": "string (Solana Public Key)",
    "bet_lamports": 1000000
  }
  ```
- **Response**:
  ```json
  {
    "success": true,
    "match_id": "UUID-string",
    "initial_state": {
      "player_health": 3,
      "dealer_health": 3,
      "shells_remaining": 6,
      "live_shells": 3,
      "blank_shells": 3,
      "items": {
        "magnifyingGlass": 1,
        "beer": 0,
        "handcuffs": 0,
        "cigarettes": 0,
        "saw": 0,
        "pill": 0
      },
      "dealer_items": {
        "magnifyingGlass": 0,
        "beer": 1,
        "handcuffs": 0,
        "cigarettes": 0,
        "saw": 0,
        "pill": 0
      },
      "is_player_turn": true
    }
  }
  ```

### 3. Process Player Action
Handles all player moves (Shooting, Item usage, or Folding).
- **URL**: `/match/{match_id}/action`
- **Method**: `POST`
- **Request Body**:
  ```json
  {
    "match_id": "string",
    "player_wallet": "string",
    "action": "ShootDealer | ShootSelf | UseItem | Fold",
    "item_type": "magnifyingGlass | beer | handcuffs | cigarettes | saw | pill (optional)"
  }
  ```
- **Response**:
  ```json
  {
    "success": true,
    "state_update": {
      "player_health": 2,
      "dealer_health": 3,
      "shells_remaining": 5,
      "live_shells": 2,
      "blank_shells": 3,
      "items": { ... },
      "dealer_items": { ... },
      "is_player_turn": false,
      "game_status": "playing | round_end | gameover",
      "chamber_peek": "live | blank (optional, only if magnifyingGlass used)",
      "last_action_result": {
        "type": "ShootSelf",
        "is_live": true,
        "damage": 1,
        "item_effect": "saw_active (optional)"
      }
    }
  }
  ```

### 4. Get Dealer Turn
Calculates the AI Dealer's actions when it is the Dealer's turn.
- **URL**: `/match/{match_id}/dealer-turn`
- **Method**: `POST`
- **Request Body**:
  ```json
  {
    "match_id": "string",
    "player_health": 3,
    "dealer_health": 3,
    "shells_remaining": 6,
    "live_shells": 3,
    "blank_shells": 3,
    "items": { ... },
    "player_handcuffed": false
  }
  ```
- **Response**:
  ```json
  {
    "success": true,
    "actions": [
      {
        "type": "UseItem",
        "item": "saw",
        "result": "Dealer used Saw"
      },
      {
        "type": "ShootPlayer",
        "is_live": true,
        "damage": 2
      }
    ]
  }
  ```

### 5. Get Player History
Retrieves a list of previous matches for a specific wallet.
- **URL**: `/player/{wallet}/history`
- **Method**: `GET`
- **Response**:
  ```json
  {
    "success": true,
    "history": [
      {
        "_id": "UUID",
        "room_pubkey": "string",
        "player1": "wallet1",
        "player2": "wallet2",
        "winner": "winner_wallet",
        "total_bet": 1000000,
        "started_at": "ISO-8601",
        "ended_at": "ISO-8601"
      }
    ]
  }
  ```

### 6. Get Match Details
Retrieves the sequence of moves for a specific match.
- **URL**: `/match/{match_id}/details`
- **Method**: `GET`
- **Response**:
  ```json
  {
    "success": true,
    "details": [
      {
        "_id": "UUID",
        "match_id": "UUID",
        "player_wallet": "string",
        "action": "ShootDealer",
        "item_type": null,
        "result": "Shot dealer with live shell",
        "created_at": "ISO-8601"
      }
    ]
  }
  ```

---

## Data Structures

### Game Status
- `playing`: Round is ongoing.
- `round_end`: Dealer hit 0 health (Player wins round).
- `gameover`: Player hit 0 health or Folded (Player loses).

### Shell Type
- `live`: Live shell.
- `blank`: Blank shell.

### Items
- `magnifyingGlass`: Peeks at the current shell.
- `beer`: Ejects the current shell.
- `handcuffs`: Skips the opponent's next turn.
- `cigarettes`: Restores 1 health.
- `saw`: Doubles the damage of the next live shell.
- `pill`: 50/50 chance to heal or damage.
