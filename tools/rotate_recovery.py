#!/usr/bin/env python3
"""Rotate an account recovery phrase directly in the database.

This script does NOT touch the application source. It only:
  1. Generates (or accepts) a new 12-word BIP39 recovery phrase.
  2. Hashes it with Argon2id using the same parameters as the server.
  3. Updates `users.recovery_hash` for the target username.
  4. Prints the words to re-import in the client.

Requirements:
  pip install argon2-cffi mnemonic            # always
  pip install psycopg[binary]                 # only for PostgreSQL

The server stores only the Argon2id hash. This script does not recover or reveal
an existing phrase. It replaces the stored hash so that the NEW phrase becomes
the one accepted by `POST /api/auth/recover`.

WARNING: changing the recovery phrase changes the client `masterSecret`, which
derives the roster encryption key. The existing encrypted roster becomes
unreadable unless it is decrypted with the OLD words first and re-uploaded.
Do NOT regenerate the ML-KEM/ML-DSA/ECDSA prekey if you want to keep existing
friend links. Re-import the printed words in the client afterward.
"""

import argparse
import os
import sqlite3
import sys

try:
    from argon2 import PasswordHasher, Type
    from argon2.exceptions import HashingError
except ImportError:
    sys.exit("Missing dependency. Run: pip install argon2-cffi mnemonic")


def generate_words() -> list[str]:
    try:
        from mnemonic import Mnemonic
    except ImportError:
        sys.exit("Missing dependency for word generation. Run: pip install mnemonic")
    return Mnemonic("english").generate(strength=128).split()


def normalize_phrase(words: list[str]) -> str:
    return " ".join(w.strip().lower() for w in words if w.strip())


def hash_phrase(phrase: str) -> str:
    hasher = PasswordHasher(
        time_cost=2,
        memory_cost=19456,  # 19 MiB, matches Argon2::default()
        parallelism=1,
        hash_len=32,
        type=Type.ID,
    )
    return hasher.hash(phrase)


def update_sqlite(path: str, username: str, phc: str) -> int:
    conn = sqlite3.connect(path)
    try:
        cur = conn.execute(
            "UPDATE users SET recovery_hash = ? WHERE username = ?",
            (phc, username),
        )
        conn.commit()
        return cur.rowcount
    finally:
        conn.close()


def update_postgres(url: str, username: str, phc: str) -> int:
    try:
        import psycopg
    except ImportError:
        sys.exit("Missing dependency for PostgreSQL. Run: pip install 'psycopg[binary]'")
    with psycopg.connect(url) as conn:
        with conn.cursor() as cur:
            cur.execute(
                "UPDATE users SET recovery_hash = %s WHERE username = %s",
                (phc, username),
            )
            return cur.rowcount


def update_database(db: str, username: str, phc: str) -> int:
    if db.startswith("postgres://") or db.startswith("postgresql://"):
        return update_postgres(db, username, phc)
    if db.startswith("sqlite://"):
        path = db[len("sqlite://"):]
    else:
        path = db
    if not path:
        sys.exit("Empty SQLite path.")
    return update_sqlite(path, username, phc)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--username", required=True, help="Account username (lowercase).")
    parser.add_argument(
        "--words",
        nargs="*",
        help="Optional 12-word phrase. If omitted, a new phrase is generated.",
    )
    parser.add_argument(
        "--db",
        default=os.environ.get("DB_URL", "sqlite://files/qxp.sqlite"),
        help="Database URL (sqlite://... or postgres://...), or a SQLite file path.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print the phrase and hash without writing to the database.",
    )
    args = parser.parse_args()

    words = list(args.words) if args.words else generate_words()
    if len(words) != 12:
        sys.exit("A recovery phrase must contain exactly 12 words.")

    phrase = normalize_phrase(words)
    try:
        phc = hash_phrase(phrase)
    except HashingError as err:
        sys.exit(f"Argon2id hashing failed: {err}")

    username = args.username.strip().lower()

    print("Username:      ", username)
    print("Recovery words:", " ".join(words))
    print("Argon2id hash: ", phc)
    print()

    if args.dry_run:
        print("Dry run: no database write performed.")
        print("To apply, re-run without --dry-run.")
        return 0

    try:
        affected = update_database(args.db, username, phc)
    except Exception as err:  # noqa: BLE001 - report any DB error clearly
        sys.exit(f"Database update failed: {err}")

    if affected == 0:
        print("WARNING: no row was updated. Check the username and database path.")
        return 1

    print(f"OK: updated {affected} row(s).")
    print()
    print("Next steps:")
    print("  1. Re-import the printed words in the client (Security settings).")
    print("  2. Expect the encrypted roster to be unreadable unless it was")
    print("     decrypted with the old words and re-uploaded first.")
    print("  3. Do NOT regenerate the ML-KEM/ML-DSA/ECDSA prekey if you want to")
    print("     keep existing friend links.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
