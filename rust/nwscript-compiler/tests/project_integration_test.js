const { WasmCompiler } = require('../pkg/nwscript_compiler.js');
const fs = require('fs');
const path = require('path');

const c = new WasmCompiler();
c.setLanguageSpec(fs.readFileSync('/home/philippechab/github/fru/nwscript.nss', 'utf-8'));
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

// Load ALL .nss files from project
const allFiles = [];
function loadAll(dir) {
    for (const e of fs.readdirSync(dir, {withFileTypes:true})) {
        const full = path.join(dir, e.name);
        if (e.isDirectory()) loadAll(full);
        else if (e.name.endsWith('.nss') && e.name !== 'nwscript.nss') {
            const name = path.basename(e.name, '.nss');
            c.addFile(name, fs.readFileSync(full, 'utf-8'));
            allFiles.push(name);
        }
    }
}
loadAll('/home/philippechab/github/fru/src');

// Also load stock NWN scripts referenced by the project.
// In production the LSP resolves these from the NWN installation.
// Here we provide stubs for the ones we know about.
const stockStubs = {
    'x0_i0_stringlib': `
string GetTokenByPosition(string sText, string sDelimiter, int nPosition);
int GetNumberTokens(string sText, string sDelimiter);
`,
};
for (const [name, content] of Object.entries(stockStubs)) {
    c.addFile(name, content);
}

console.log(`Loaded ${allFiles.length} project files + ${Object.keys(stockStubs).length} stock stubs\n`);

let totalErrors = 0;
let failedFiles = [];
for (const name of allFiles) {
    c.compile(name);
    const n = c.getCollectedErrorCount();
    if (n > 0) {
        failedFiles.push(name);
        totalErrors += n;
        console.log(`FAIL ${name} (${n} errors)`);
        for (let i = 0; i < Math.min(n, 5); i++) {
            console.log(`  ${c.getCollectedError(i)}`);
        }
        if (n > 5) console.log(`  ... and ${n-5} more`);
    }
}

console.log(`\n${allFiles.length - failedFiles.length}/${allFiles.length} passed, ${totalErrors} total errors`);
c.free();

// Known-bad files: real bugs in the user's project, kept as regression checks.
// "consts_roles": contains `string UNDEFINED_STRING = 1;` (int → string mismatch).
const knownBad = new Set(['consts_roles']);
const unexpected = failedFiles.filter(f => !knownBad.has(f));
if (unexpected.length) {
    console.log(`UNEXPECTED failures: ${unexpected.join(', ')}`);
    process.exit(1);
}
process.exit(0);
