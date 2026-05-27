const { WasmCompiler } = require('./pkg/nwscript_compiler.js');
const fs = require('fs');
const path = require('path');

const c = new WasmCompiler();
const spec = fs.readFileSync('/home/philippechab/github/fru/nwscript.nss', 'utf-8');
c.setLanguageSpec(spec);
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

// Load a real project file with includes
const srcDir = '/home/philippechab/github/fru/src';

function loadNssFiles(dir) {
    const entries = fs.readdirSync(dir, {withFileTypes: true});
    for (const e of entries) {
        const full = path.join(dir, e.name);
        if (e.isDirectory()) loadNssFiles(full);
        else if (e.name.endsWith('.nss')) {
            const name = path.basename(e.name, '.nss');
            c.addFile(name, fs.readFileSync(full, 'utf-8'));
        }
    }
}
loadNssFiles(srcDir);

// Try compiling a file that uses json and includes
const testFiles = ['core_cmds', 'nwnx', 'nwnx_regex'];
for (const name of testFiles) {
    c.compile(name);
    const n = c.getCollectedErrorCount();
    console.log(`${name}: ${n === 0 ? 'PASS' : 'FAIL'} (${n} errors)`);
    for (let i = 0; i < Math.min(n, 3); i++) {
        console.log(`  ${c.getCollectedError(i)}`);
    }
}
c.free();
