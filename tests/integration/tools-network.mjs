// Opt-in network check for auto-installed binaries. A version check cannot detect DNS failures.
import {spawnSync} from 'node:child_process';
import {mkdtemp} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import assert from 'node:assert/strict';
const home=await mkdtemp(join(tmpdir(),'ytlr-engine-network-'));
const cli=resolve(`target/release/ytlr${process.platform==='win32'?'.exe':''}`);
const run=(args)=>{const r=spawnSync(cli,['--data-dir',home,...args],{encoding:'utf8',timeout:240000});assert.equal(r.status,0,r.stderr);return r.stdout;};
try{
  run(['tools','install']);
  const status=JSON.parse(run(['doctor']));
  assert(status.tools.every(t=>!t.error));
  const ffmpeg=status.tools.find(t=>t.name==='ffmpeg');
  const test=spawnSync(ffmpeg.path,['-v','error','-rw_timeout','10000000','-i','https://example.com','-f','null','-'],{encoding:'utf8',timeout:20000});
  assert(/Invalid data|HTTP error/.test(test.stderr),`DNS/HTTPS transport failed: ${test.stderr}`);
  console.log(`PASS: auto-installed tools execute; FFmpeg DNS and HTTPS work (${ffmpeg.version})`);
}finally{spawnSync(cli,['--data-dir',home,'shutdown'],{encoding:'utf8',timeout:15000});}
