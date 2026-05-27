const { WasmCompiler } = require('./pkg/nwscript_compiler.js');
const fs = require('fs');
const c = new WasmCompiler();
c.setLanguageSpec(fs.readFileSync('/home/philippechab/github/fru/nwscript.nss', 'utf-8'));
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

// Check the actual problem lines
const coreLgs = fs.readFileSync('/home/philippechab/github/fru/src/systems/languages/core_lgs.nss', 'utf-8');
console.log('core_lgs line 137:', coreLgs.split('\n')[136]?.trim());
console.log('core_lgs line 161:', coreLgs.split('\n')[160]?.trim());

const nwnxEffect = fs.readFileSync('/home/philippechab/github/fru/src/nwnx/nwnx_effect.nss', 'utf-8');
console.log('\nnwnx_effect line 252:', nwnxEffect.split('\n')[251]?.trim());
console.log('nwnx_effect line 253:', nwnxEffect.split('\n')[252]?.trim());

const runSc = fs.readFileSync('/home/philippechab/github/fru/src/cli/commands/player/run_sc.nss', 'utf-8');
console.log('\nrun_sc line 14:', runSc.split('\n')[13]?.trim());

const webhook = fs.readFileSync('/home/philippechab/github/fru/src/nwnx/nwnx_webhook_rch.nss', 'utf-8');
console.log('\nnwnx_webhook_rch line 94:', webhook.split('\n')[93]?.trim());
console.log('nwnx_webhook_rch line 106:', webhook.split('\n')[105]?.trim());

const skillranks = fs.readFileSync('/home/philippechab/github/fru/src/nwnx/nwnx_skillranks.nss', 'utf-8');
console.log('\nnwnx_skillranks line 232:', skillranks.split('\n')[231]?.trim());

// Test specific patterns
console.log('\n--- Pattern tests ---');

// string + string  
c.addFile('t1', 'void main() { string s = "a" + "b"; }');
c.compile('t1'); 
console.log('string+string:', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');

// int to string concat
c.clearFiles();
c.addFile('t2', 'string IntToString(int n);\nvoid main() { string s = "x" + IntToString(5); }');
c.compile('t2');
console.log('string+fn():', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log('  ', c.getCollectedError(i));

c.free();
