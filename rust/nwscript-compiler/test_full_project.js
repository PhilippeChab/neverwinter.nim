const { WasmCompiler } = require('./pkg/nwscript_compiler.js');
const fs = require('fs');
const path = require('path');

const c = new WasmCompiler();
c.setLanguageSpec(fs.readFileSync('/home/philippechab/github/fru/nwscript.nss', 'utf-8'));
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

// Load ALL .nss files from the project (simulating what the LSP does)
function loadAll(dir) {
    for (const e of fs.readdirSync(dir, {withFileTypes:true})) {
        const full = path.join(dir, e.name);
        if (e.isDirectory()) loadAll(full);
        else if (e.name.endsWith('.nss') && e.name !== 'nwscript.nss') {
            c.addFile(path.basename(e.name, '.nss'), fs.readFileSync(full, 'utf-8'));
        }
    }
}
loadAll('/home/philippechab/github/fru/src');

// Compile core_cmds which uses json + struct + includes
const code = c.compile('core_cmds');
const n = c.getCollectedErrorCount();
console.log(`core_cmds: ${n === 0 ? 'PASS' : 'FAIL'} (${n} errors)`);
for (let i = 0; i < Math.min(n, 10); i++) {
    console.log('  ' + c.getCollectedError(i));
}
c.free();
