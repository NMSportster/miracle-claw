import { readFileSync } from "node:fs";
const f = "/home/adeal/.openclaw/workspace/projects/miracle-claw/src-tauri/resources/dist/gateway-CvsJ0gY0.js";
const src = readFileSync(f, "utf8");
const start = src.indexOf('const RETRY_GUIDANCE_SUFFIX');
const end = src.indexOf('//#region src/agents/before-tool-call-metadata.ts');
const region = src.slice(start, end).replace(/export\s+/g, "");
const m = await import("data:text/javascript;base64," + Buffer.from(region + "\nexport { assertRequiredParams, REQUIRED_PARAM_GROUPS };").toString("base64"));

let pass=0, fail=0;
const t=(n,fn,ok)=>{try{fn();if(ok){console.log("  PASS  "+n);pass++}else{console.log("  FAIL  "+n);fail++}}catch(e){if(!ok){console.log("  PASS  "+n+" -> "+e.message.slice(0,60));pass++}else{console.log("  FAIL  "+n+" -> "+e.message);fail++}}};
const W=m.REQUIRED_PARAM_GROUPS.write;

console.log("\nAgainst the REAL patched bundle in src-tauri/resources/dist:");
console.log("--- THE FIX ---");
t('content:"" + valid path -> ACCEPTED', ()=>m.assertRequiredParams({path:"C:/x/__init__.py",content:""},W,"write"), true);
t('content:"\\n"           -> ACCEPTED', ()=>m.assertRequiredParams({path:"C:/x/a.py",content:"\n"},W,"write"), true);
console.log("--- MUST STILL REJECT ---");
t('content absent           -> REJECTED', ()=>m.assertRequiredParams({path:"C:/x/a.py"},W,"write"), false);
t('content:null             -> REJECTED', ()=>m.assertRequiredParams({path:"C:/x/a.py",content:null},W,"write"), false);
t('path:"" (still required)  -> REJECTED', ()=>m.assertRequiredParams({path:"",content:""},W,"write"), false);
t('path absent              -> REJECTED', ()=>m.assertRequiredParams({content:"x"},W,"write"), false);
console.log("--- NO REGRESSION ---");
t('read path:""             -> REJECTED', ()=>m.assertRequiredParams({path:""},m.REQUIRED_PARAM_GROUPS.read,"read"), false);
t('read path:"f.md"         -> ACCEPTED', ()=>m.assertRequiredParams({path:"f.md"},m.REQUIRED_MOD_GROUPS?.read ?? m.REQUIRED_PARAM_GROUPS.read,"read"), true);
t('edit edits:[]            -> REJECTED', ()=>m.assertRequiredParams({path:"f.txt",edits:[]},m.REQUIRED_PARAM_GROUPS.edit,"edit"), false);
t('edit newText:""          -> ACCEPTED', ()=>m.assertRequiredParams({path:"f.txt",edits:[{oldText:"a",newText:""}]},m.REQUIRED_PARAM_GROUPS.edit,"edit"), true);
console.log(`\n=== ${pass} passed, ${fail} failed ===\n`);
process.exit(fail?1:0);
