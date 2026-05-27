const { WasmCompiler } = require('./pkg/nwscript_compiler.js');
const fs = require('fs');
const c = new WasmCompiler();
c.setLanguageSpec(fs.readFileSync('/home/philippechab/github/fru/nwscript.nss', 'utf-8'));
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

// 1. "Arithmetic operation has invalid operands" - string + string?
c.addFile('t1', 'void main() { string s = "a" + "b"; }');
c.compile('t1');
console.log('string concat:', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log('  ', c.getCollectedError(i));

// 2. Check the actual line from core_lgs
const coreLgs = fs.readFileSync('/home/philippechab/github/fru/src/languages/core_lgs.nss', 'utf-8');
const line137 = coreLgs.split('\n')[136];
console.log('\ncore_lgs line 137:', line137?.trim());

// 3. "Left side of '.' is not a structure" - check nwnx_effect line 252
const nwnxEffect = fs.readFileSync('/home/philippechab/github/fru/src/nwnx/nwnx_effect.nss', 'utf-8');
const lines = nwnxEffect.split('\n');
console.log('\nnwnx_effect line 252:', lines[251]?.trim());
console.log('nwnx_effect line 253:', lines[252]?.trim());

// 4. "Unexpected character" - check run_sc line 14
const runSc = fs.readFileSync('/home/philippechab/github/fru/src/cli/run_sc.nss', 'utf-8');
console.log('\nrun_sc line 14:', runSc.split('\n')[13]?.trim());
console.log('run_sc line 15:', runSc.split('\n')[14]?.trim());

// 5. Test vector operations since that's likely the arithmetic issue
c.clearFiles();
c.addFile('t2', 'void main() { vector v = Vector(1.0, 2.0, 3.0); float x = v.x; }');
c.compile('t2');
console.log('\nvector.x access:', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log('  ', c.getCollectedError(i));

// 6. Test string + int (IntToString pattern)
c.clearFiles();
c.addFile('t3', 'void main() { string s = "count: " + IntToString(5); }');
c.compile('t3');
console.log('\nstring + IntToString:', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log('  ', c.getCollectedError(i));

c.free();
