Nouveaux changement majeur dans les ROOMS:
Système de permissions.

Actuellement , n'importe qu'elle personne qui rejoins une room à accès à:
- Changer l'îcone
- Changer le nom
- parler dedans

Je voudrais qu'on créer une nouvelle philosphie :
Un système de salon "dit" communautaire.

Ce que font les salons communautaire:

Il intègre un système de permission avancées:

- Le créateur du channel est enrollé "Administrateur"
- Les membres rejoignont le salon sont enrollé "Membre"
- L'administrateur peut enrollé jusqu'à 5 modérateur (Limite maximal définis en amont par le protocol)
- L'administrateur peut enrollé jusqu'à 3 Sous-Administrateur (Limite maximal définis en amont par le protocol)

Les rôles eux, sont sauvegardée de manière permanante dans la base de donnés.
    - On sauvegarderas les identifiants utilisateurs pour les rôles, et non pas les @username.

Ces salons dit communautaire verront 3 sous-catégorie de permissions hierarchique. avec plusieurs paramètres.

Un salon "dit" communautaire à 3 niveaux de permissions:

Administrateur:
- Peut parler
- changer de nom à la "room"
- changer l'avatar de la room
- bannir quelqu'un de la room
    -> Car oui, les salons communautaire vont intégréer un système de banissement auto-gérée par les modérateurs/administrateur de la room
        -> Stocké de manière persistante avec les identifiants utilisateurs des membres bannis.
- Peuvent désactiver la communication dans le chat
    -> Car oui, nouveaux changement profond, une room "communautaire" peut "juste" servir de canal de communication pour un certain type de membre (Administrateur en l'occurence)
- rendre muet des membres pour un certaint temps
    -> Car oui, tu vas integrer un système de "timeout" temporaire, où un membre n'a plus la permissions de parler pendant X temps attribuée par un Administrateur / Modérateur. 
- supprimer des messages
    -> Car oui, notion importante, les salons dit "communautaire" , les messages sont "managable" en live par les Administrateur / modérateur
- décider de changer la propriété de sont salon à quelqu'un d´autre
    -> Le protocol imposeras que la personne qui récupère la propriété du channel doit être en amont Modérateur/Sous-Admin

Sous-Adminisrateur:
(Même permissions que l'administrateur, peuvent juste ce faire manager par l'administrateur. Les sous-admin ne peuvent pas faire d'actions de modération sur l'admin)

Modérateur:
- Peut bannir des membres
- expulser des membres
- rendre muet des membres
- supprimer des messages.

Situation actuel:
Quand on clique sur le bouton class="icon-btn side__shuffle"
Le client créer une "room" qui est absolument pas adapté à ce qu'on veut faire

Ma volonté, quand ont cliqueras sur ce bouton, cela ouvriras un grand grand menue où:

- On choisie le type de salon (room classique , room communautaire)
- Le nom, l'avatar
- la description de la room (qui s'afficheras de le thread .class="thread__sub")
    -> Car oui , maintenant dans les rooms ont pourras mettre une description de moins de 140 char à la place de "Room conversation - E2EE Ready"
- les permissions si modérateur

Tu vas adapter toute la stack (protocol, schéma de db, client) pour acceuilir toute les fonctionalités.
Si à un moment, tu ne comprend pas quelquee choses, tu ne réinvente rien, tu me poses toute les questions nécessaire.
