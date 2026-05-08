# Shellshock Relay

Relay off-chain para `Shell Shock`. Este servicio no firma transacciones ni decide resultados. Coordina matchmaking PvP, observa rooms on-chain y reemite eventos públicos a la UI.

## Endpoints
- `GET /healthz`
- `GET /readyz`
- `GET /config`
- `GET /ws`

## Variables de entorno
Usa [`./.env.example`](./.env.example) como base.

```env
PORT=8080
SOLANA_RPC_HTTP_URL=https://api.devnet.solana.com
SOLANA_RPC_WS_URL=wss://api.devnet.solana.com
PROGRAM_ID=11111111111111111111111111111111
CORS_ORIGIN=http://localhost:5173
TURN_TIMEOUT_SECONDS=90
```

## Arranque local
```powershell
cd shellshock-api
cargo run
```

```powershell
cd shellshock-ui
npm install --ignore-scripts
npm run dev
```

## Docker
```powershell
cd shellshock-api
docker build -t shellshock-relay .
docker run --rm -p 8080:8080 --env-file .env shellshock-relay
```

## Runbook corto
- Verifica `GET /readyz` antes de abrir la UI PvP.
- Confirma que `PROGRAM_ID` coincide con el deploy devnet del programa.
- Si se cae el websocket RPC, el observer reintenta en bucle.
- Si el relay se reinicia, las colas en memoria se pierden; las rooms on-chain se pueden reanexar con `session.resume`.

## Verificacion local
- `npm run build` en `shellshock-ui` pasa.
- `cargo fmt --check` en `shellshock-api` pasa.
- `cargo check` del relay no se pudo completar en esta maquina porque el host Windows no tiene `link.exe` para MSVC ni `dlltool.exe` para GNU.
