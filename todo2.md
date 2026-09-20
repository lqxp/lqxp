Ajoute un Discord Rich Presence au client desktop (Pc) QxChat:
- Avec paramètre spécifique dans : "Avancée" catégorie complète pour activer/desactiver/modifier/paramètre le RPC.
  - Activer par défaut.

Le RPC Client ID: 1548385894283608145
rpc assets:
icon
linux
macos
windows

Je veux que sa utilise l'IPC via le backend Rust du client Tauri pour appeler ensuite Discord.
Faut que sa fonctionne sur macOS, Linux , Windows nativement. Donc trouve une vrai bonne bonne libraries Rust pour rpc.
Je veux un RPC comme sa:
Grosse Image: icon (tooltip text: QxChat v<Version>)
Petite Image: macos/linux/windows (auto détéction) (tooltip: on <CapitalizedPlatform>)
Activity: Playing QxChat

Settings du RPC:
  - Activer/desactiver
  - desactiver petite image detection OS

Donc relier backend rust + paramètre front-end
