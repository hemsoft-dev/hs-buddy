import { readFileSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'esbuild'

const directory = resolve(process.argv[2] ?? '')
if (
  readFileSync(join(directory, '.buddy-settings-fixture'), 'utf8') !== 'buddy settings fixture v1'
) {
  throw new Error('An owned settings fixture directory is required')
}
const modulePath = join(directory, 'config-companion-module.mjs')
await build({
  entryPoints: ['electron/config.ts'],
  outfile: modulePath,
  bundle: true,
  platform: 'node',
  format: 'esm',
  external: ['electron'],
  define: { 'import.meta.env.VITE_CONVEX_URL': 'undefined' },
  banner: {
    js: "import {createRequire as __fixtureRequire} from 'node:module';const require=__fixtureRequire(import.meta.url);",
  },
  plugins: [
    {
      name: 'deny-fixture-credential-access',
      setup(builder) {
        builder.onResolve({ filter: /^@napi-rs\/keyring$/ }, () => ({
          path: 'keyring',
          namespace: 'fixture',
        }))
        builder.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({
          contents:
            "export class AsyncEntry { constructor(){throw new Error('Config fixture must not access credentials')} static withTarget(){throw new Error('Config fixture must not access credentials')} }",
          loader: 'js',
        }))
      },
    },
  ],
})
const bootstrap = `import {app} from 'electron';
import {readFileSync,writeFileSync} from 'node:fs';
import {dirname,join,resolve} from 'node:path';
app.disableHardwareAcceleration();
const profile=dirname(resolve(process.env.BUDDY_CONFIG_PATH));
if(readFileSync(join(profile,'.buddy-settings-fixture'),'utf8')!=='buddy settings fixture v1') throw new Error('Missing fixture marker');
app.setPath('userData',profile);
app.whenReady().then(async()=>{
  if(process.platform==='darwin') app.setActivationPolicy('prohibited');
  const {configManager}=await import(${JSON.stringify(pathToFileURL(modulePath).href)});
  writeFileSync(join(profile,'companion-ready.json'),JSON.stringify({ready:true,electron:process.versions.electron,startedAt:Date.now()}));
  const started=Date.now();let writes=0;
  while(Date.now()-started<20000){
    configManager.setUiValue('enterpriseSlug','fixture-concurrent-'+writes++);
    await new Promise(resolve=>setTimeout(resolve,25));
  }
  configManager.setUiValue('enterpriseSlug','fixture-concurrent-complete');
  writeFileSync(join(profile,'companion-complete.json'),JSON.stringify({result:'passed',writes,electron:process.versions.electron,credentialAccess:false,completedAt:Date.now()}));
  app.exit(0);
}).catch(error=>{
  writeFileSync(join(dirname(resolve(process.env.BUDDY_CONFIG_PATH)),'companion-failed.json'),JSON.stringify({stage:'fixture config initialization or write',type:error?.name,message:error?.message}));
  console.error('The owned Electron config companion failed. Credential values are withheld.');
  app.exit(1);
});
`
writeFileSync(join(directory, 'config-companion.mjs'), bootstrap)
writeFileSync(
  join(directory, 'package.json'),
  JSON.stringify({
    name: 'buddy-settings-companion',
    version: '0.0.0',
    main: 'config-companion.mjs',
    type: 'module',
  })
)
console.log('Built the owned Electron config companion with credential access disabled.')
