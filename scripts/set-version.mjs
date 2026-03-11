/**
 * Reads the version from package.json and stamps it into Cargo.toml.
 * Run with: node scripts/set-version.mjs
 */
import { readFileSync, writeFileSync } from "fs";

const pkg = JSON.parse(readFileSync("package.json", "utf8"));
const version = pkg.version;

// Update src-tauri/Cargo.toml
const cargoPath = "src-tauri/Cargo.toml";
let cargo = readFileSync(cargoPath, "utf8");
cargo = cargo.replace(/^version = "[^"]*"/m, `version = "${version}"`);
writeFileSync(cargoPath, cargo);

console.log(`Version set to ${version}`);
