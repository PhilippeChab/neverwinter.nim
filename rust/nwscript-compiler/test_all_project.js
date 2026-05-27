const { WasmCompiler } = require('./pkg/nwscript_compiler.js');
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
            const content = fs.readFileSync(full, 'utf-8');
            c.addFile(name, content);
            allFiles.push(name);
        }
    }
}
loadAll('/home/philippechab/github/fru/src');

// Also load nwscript.nss content as a file for includes that reference it
// And x0_i0_stringlib if it exists in the project
const nwscriptContent = fs.readFileSync('/home/philippechab/github/fru/nwscript.nss', 'utf-8');
c.addFile('nwscript', nwscriptContent);

console.log(`Loaded ${allFiles.length} files\n`);

// Compile every file and collect results
let totalErrors = 0;
let failedFiles = [];
for (const name of allFiles) {
    const code = c.compile(name);
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
if (failedFiles.length > 0) {
    // Collect unique error types
    const errorTypes = new Set();
    for (const name of failedFiles) {
        c.compile(name);
        for (let i = 0; i < c.getCollectedErrorCount(); i++) {
            const err = c.getCollectedError(i);
            const match = err.match(/ERROR:\s*(.+?)(?::|$)/);
            if (match) errorTypes.add(match[1].trim());
        }
    }
    console.log('\nUnique error types:');
    for (const t of errorTypes) console.log(`  - ${t}`);
}
c.free();
