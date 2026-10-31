// hs-buddy Aspire AppHost
// Orchestrates the Convex backend and Vite+Electron frontend
// Run with: aspire run (or: bun run aspire)
// Docs: https://aspire.dev

import { createBuilder } from './.aspire/modules/aspire.mjs'

// Aspire SDK transport layer depends on this.
import 'vscode-jsonrpc'

const builder = await createBuilder()

// Convex local dev server (backend on port 3210, dashboard on 6790)
const convex = await builder
  .addJavaScriptApp('convex', '..')
  .withBun({ install: false })
  .withRunScript('convex:dev')

// Vite dev server + Electron (vite-plugin-electron handles Electron launch)
await builder
  .addViteApp('buddy', '..')
  .withBun({ install: false })
  .withReference(convex)
  .waitFor(convex)
  .withExternalHttpEndpoints()
  .withOtlpExporter()

// Opt in alongside Electron. Electron remains the primary/default app.
if (process.env.BUDDY_ASPIRE_NATIVE === '1') {
  await builder
    .addJavaScriptApp('buddy-native', '..')
    .withBun({ install: false })
    .withRunScript('native:dev')
    .withReference(convex)
    .waitFor(convex)
    .withOtlpExporter()
}

await builder.build().run()
