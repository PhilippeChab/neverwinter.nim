const { WasmCompiler } = require('./pkg/nwscript_compiler.js');
const fs = require('fs');
const c = new WasmCompiler();
c.setLanguageSpec(fs.readFileSync('/home/philippechab/github/fru/nwscript.nss', 'utf-8'));
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

// Exact struct from project
c.addFile('t1', `struct commandStruct { string sCommand; string sAlias; int nRole; string sScript; string sLabel; };
json foo(struct commandStruct command) { }`);
c.compile('t1');
console.log('t1:', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log(' ', c.getCollectedError(i));

// Check if "command" as param name conflicts with the "command" engine structure keyword
c.clearFiles();
c.addFile('t2', 'void foo(int command) { }');
c.compile('t2');
console.log('t2 (param named "command"):', c.getCollectedErrorCount() === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) console.log(' ', c.getCollectedError(i));

c.free();
