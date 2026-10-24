// Print the licenses of the npm packages bundled into Cyze's frontend: the
// production dependency tree, each package once, with its license file.
// Used by third-party-notices.sh.
import { execSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

// Through a shell, so Windows finds npm's .cmd shim.
const tree = JSON.parse(execSync("npm ls --omit=dev --all --long --json", { encoding: "utf8", maxBuffer: 64 << 20 }));
const packages = new Map();
(function walk(deps) {
  for (const [name, dep] of Object.entries(deps ?? {})) {
    if (!dep.path || dep.extraneous) continue;
    const key = `${name}@${dep.version}`;
    if (!packages.has(key)) packages.set(key, { name, version: dep.version, path: dep.path });
    walk(dep.dependencies);
  }
})(tree.dependencies);

const licenseFile = (dir) =>
  readdirSync(dir).find((f) => /^(licen[cs]e|copying)(\.(md|txt))?$/i.test(f) || /^licen[cs]e-/i.test(f));

let missing = 0;
for (const p of [...packages.values()].sort((a, b) => a.name.localeCompare(b.name))) {
  const pkg = JSON.parse(readFileSync(join(p.path, "package.json"), "utf8"));
  const license = typeof pkg.license === "string" ? pkg.license : (pkg.license?.type ?? "UNKNOWN");
  const file = licenseFile(p.path);
  const repo = typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url;
  console.log("-".repeat(80));
  console.log(`${p.name} ${p.version}: ${license}${repo ? ` (${repo.replace(/^git\+/, "")})` : ""}`);
  console.log();
  if (file) {
    console.log(readFileSync(join(p.path, file), "utf8").trim());
  } else {
    missing++;
    console.log(`No license file is included in the package; see its ${license} license at its repository.`);
  }
  console.log();
}
if (missing) console.error(`npm-notices: ${missing} package(s) ship without a license file`);
