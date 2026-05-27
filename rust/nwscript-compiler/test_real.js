const { WasmCompiler } = require('./pkg/nwscript_compiler.js');
const fs = require('fs');

const c = new WasmCompiler();

const spec = fs.readFileSync('/home/philippechab/github/fru/nwscript.nss', 'utf-8');
c.setLanguageSpec(spec);
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

// Test 1: json type
c.addFile('test', 'void main() { json j = JsonObject(); }');
let code = c.compile('test');
console.log('Test 1 (json j = JsonObject()):', code === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) {
    console.log('  Error:', c.getCollectedError(i));
}

// Test 2: effect type
c.clearFiles();
c.addFile('test2', 'void main() { effect e = EffectDamage(10); }');
code = c.compile('test2');
console.log('Test 2 (effect e):', code === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) {
    console.log('  Error:', c.getCollectedError(i));
}

// Test 3: nwnx.nss from real project
const nwnxNss = fs.readFileSync('/home/philippechab/github/fru/src/nwnx/nwnx.nss', 'utf-8');
c.clearFiles();
c.addFile('nwnx', nwnxNss);
code = c.compile('nwnx');
console.log('Test 3 (nwnx.nss):', code === 0 ? 'PASS' : 'FAIL', '(' + c.getCollectedErrorCount() + ' errors)');
for (let i = 0; i < Math.min(c.getCollectedErrorCount(), 10); i++) {
    console.log('  Error:', c.getCollectedError(i));
}

c.free();

// Debug: check if any functions were loaded
const c2 = new WasmCompiler();
const spec2 = fs.readFileSync('/home/philippechab/github/fru/nwscript.nss', 'utf-8');
c2.setLanguageSpec(spec2);
c2.setRequireEntryPoint(false);
c2.setCollectAllErrors(true);

// Try calling a function that should exist
c2.addFile('dbg', 'void main() { int x = Random(10); }');
let r = c2.compile('dbg');
console.log('\nDebug - Random(10):', r === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c2.getCollectedErrorCount(); i++) {
    console.log('  Error:', c2.getCollectedError(i));
}

// Check what the lang spec parser thinks
c2.clearFiles();
c2.addFile('dbg2', 'void main() { PrintString("hello"); }');
r = c2.compile('dbg2');
console.log('Debug - PrintString():', r === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c2.getCollectedErrorCount(); i++) {
    console.log('  Error:', c2.getCollectedError(i));
}
c2.free();
