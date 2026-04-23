# QxProtocol

Serveur de presence/match/chat entierement recentre sur WebSocket et reecrit en Rust.

## Ce qui change

- Le runtime principal est le binaire Rust `qxprotocol`.
- Le point d'entree reseau utile est `/ws`.
- Toute l'ancienne codebase TypeScript/Bun a ete retiree du depot.
- Les operations d'administration et de statut passent elles aussi par WebSocket.

## Demarrage

```bash
cargo run
```

Le serveur charge sa configuration depuis :

1. `files/config.custom.toml` si present
2. sinon `files/config.prod.toml` quand `PRODUCTION=1`
3. sinon `files/config.dev.toml`

## Configuration

```toml
[api]
domain = ""
port = 4560
adminPassword = ""

[network]
heartbeatInterval = 3000
maxConnectionsPerIp = 3
latestVersion = ""
publicDir = "serve.public"
webchatIndex = "index.html"
```

## Protocole

Le protocole historique `op` est conserve pour le gameplay/chat/voice :

- `1`: heartbeat
- `2`: identify
- `3`: join game
- `4`: leave game
- `5`: kill event
- `6`: version query
- `7`: chat
- `15`: alive exchange
- `16`: game end exchange
- `17`: exchange joined notification
- `18`: room message history sync
- `19`: toggle reaction on a message by `messageId`
- `20`: reaction update broadcast for a message
- `98`: voice chat state
- `99`: voice payload
- `100`: mute user

Operations WebSocket-first ajoutees pour remplacer les routes HTTP :

- `101`: admin status
- `102`: blacklist IP
- `103`: unblacklist IP
- `104`: broadcast admin
- `105`: online count / room count

Les reponses acceptent `d.requestId` pour faire du request/response propre sur un meme socket.

Les messages de chat sont conserves en memoire par room avec un `messageId` serveur, puis redistribues aux clients avec cet identifiant pour permettre les reactions ciblees.

## Web statique

- `/` sert `index.html`
- `/webchat` sert le client webchat
- `/app.js` et `/styles.css` sont servis depuis `serve.public/`

## Donnees

La base reste compatible avec `files/database.json` en conservant le format cle/valeur historique.
