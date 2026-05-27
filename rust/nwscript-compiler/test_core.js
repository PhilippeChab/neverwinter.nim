const { WasmCompiler } = require('./pkg/nwscript_compiler.js');
const fs = require('fs');
const c = new WasmCompiler();
c.setLanguageSpec(fs.readFileSync('/home/philippechab/github/fru/nwscript.nss', 'utf-8'));
c.setRequireEntryPoint(false);
c.setCollectAllErrors(true);

// Just the problematic part
c.addFile('test', `
struct commandStruct { string sCommand; };
json CommandToJson(struct commandStruct command)
{
    json jCommand = JsonObject();
    return jCommand;
}
`);
let code = c.compile('test');
console.log('core_cmds minimal:', code === 0 ? 'PASS' : 'FAIL');
for (let i = 0; i < c.getCollectedErrorCount(); i++) {
    console.log('  Error:', c.getCollectedError(i));
}
c.free();
