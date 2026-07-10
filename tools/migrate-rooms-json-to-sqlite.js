#!/usr/bin/env bun
import { Database } from "bun:sqlite";
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

function argValue(name, fallback) {
  const index = process.argv.indexOf(name);
  if (index >= 0 && process.argv[index + 1]) return process.argv[index + 1];
  return fallback;
}

function sqlitePath(value) {
  if (value.startsWith("sqlite://")) return value.slice("sqlite://".length);
  if (value.startsWith("sqlite:")) return value.slice("sqlite:".length);
  return value;
}

function rowValue(raw, id) {
  return Array.isArray(raw.json)
    ? raw.json.find((row) => row?.id === id)?.value
    : undefined;
}

function asRecord(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function normalizeRoom(roomId, rawRoom, iconFallback) {
  const value = asRecord(rawRoom);
  const storedRoomId = typeof value.roomId === "string" && value.roomId.trim()
    ? value.roomId.trim()
    : roomId;
  const title = typeof value.title === "string" && value.title.trim()
    ? value.title.trim()
    : storedRoomId;
  const members = Array.isArray(value.members)
    ? value.members.filter((member) => typeof member === "string")
    : [];
  const icon = value.icon ?? iconFallback ?? null;

  return {
    roomId: storedRoomId,
    title,
    icon: icon && typeof icon === "object" ? icon : null,
    members,
  };
}

const jsonPath = resolve(argValue("--json", "files/database.json"));
const sqliteFile = resolve(sqlitePath(argValue("--sqlite", "files/qxp.sqlite")));

if (!existsSync(jsonPath)) {
  throw new Error(`JSON database not found: ${jsonPath}`);
}

mkdirSync(dirname(sqliteFile), { recursive: true });

const raw = JSON.parse(readFileSync(jsonPath, "utf8"));
const roomsValue = asRecord(rowValue(raw, "rooms"));
const roomIconsValue = asRecord(rowValue(raw, "room_icons"));
const rooms = Object.entries(roomsValue).map(([roomId, room]) =>
  normalizeRoom(roomId, room, roomIconsValue[roomId]),
);

const db = new Database(sqliteFile);
db.exec(`
  CREATE TABLE IF NOT EXISTS rooms (
    room_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    icon_json TEXT,
    members_json TEXT NOT NULL DEFAULT '[]',
    updated_at BIGINT NOT NULL DEFAULT 0
  )
`);

const insert = db.prepare(`
  INSERT INTO rooms (room_id, title, icon_json, members_json, updated_at)
  VALUES ($roomId, $title, $iconJson, $membersJson, $updatedAt)
  ON CONFLICT(room_id) DO UPDATE SET
    title = excluded.title,
    icon_json = excluded.icon_json,
    members_json = excluded.members_json,
    updated_at = excluded.updated_at
`);

const now = Date.now();
const migrate = db.transaction(() => {
  for (const room of rooms) {
    insert.run({
      $roomId: room.roomId,
      $title: room.title || room.roomId,
      $iconJson: room.icon ? JSON.stringify(room.icon) : null,
      $membersJson: JSON.stringify(room.members || []),
      $updatedAt: now,
    });
  }
});

migrate();
db.close();

console.log(`Migrated ${rooms.length} rooms from ${jsonPath} to ${sqliteFile}`);
