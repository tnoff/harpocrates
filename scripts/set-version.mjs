/**
 * Reads the VERSION file and stamps the version into Cargo.toml and package.json.
 * Run with: node scripts/set-version.mjs
 */
import { readFileSync, writeFileSync } from "fs";

const version = readFileSync("VERSION", "utf8").trim();

// Update src-tauri/Cargo.toml
const cargoPath = "src-tauri/Cargo.toml";
let cargo = readFileSync(cargoPath, "utf8");
cargo = cargo.replace(/^version = "[^"]*"/m, `version = "${version}"`);
writeFileSync(cargoPath, cargo);

// Update package.json
const pkgPath = "package.json";
const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
pkg.version = version;
writeFileSync(pkgPath, JSON.stringify(pkg, null, 2) + "\n");

console.log(`Version set to ${version}`);
