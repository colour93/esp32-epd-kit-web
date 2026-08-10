import { join } from 'node:path'
import { chmod, copyFile, mkdir, rm } from 'node:fs/promises'

const root = join(import.meta.dir, '..')
const isWindows = process.platform === 'win32'

async function run(command: string, args: string[], env = process.env) {
  const child = Bun.spawn({
    cmd: [command, ...args],
    cwd: root,
    env,
    stdin: 'inherit',
    stdout: 'inherit',
    stderr: 'inherit',
  })
  const status = await child.exited
  if (status !== 0) process.exit(status)
}

await run(process.execPath, ['run', 'build'])
await run(isWindows ? 'cargo.exe' : 'cargo', ['build', '--manifest-path', 'agent/Cargo.toml'], {
  ...process.env,
  EPD_AGENT_SKIP_WEB_BUILD: '1',
})

const binary = join(root, 'agent', 'target', 'debug', isWindows ? 'epd-agent.exe' : 'epd-agent')
let launchBinary = binary
if (process.platform === 'darwin') {
  const bundle = join(root, 'agent', 'target', 'debug', 'EPD Agent.app')
  const contents = join(bundle, 'Contents')
  const bundledBinary = join(contents, 'MacOS', 'epd-agent')
  await rm(bundle, { recursive: true, force: true })
  await mkdir(join(contents, 'MacOS'), { recursive: true })
  await copyFile(binary, bundledBinary)
  await chmod(bundledBinary, 0o755)
  await copyFile(join(root, 'agent', 'macos', 'Info.plist'), join(contents, 'Info.plist'))
  await run('codesign', ['--force', '--deep', '--sign', '-', bundle])
  await run('open', ['-n', '-W', bundle, '--args', ...process.argv.slice(2)])
  process.exit(0)
}

const agent = Bun.spawn({
  cmd: [launchBinary, '--no-open', ...process.argv.slice(2)],
  cwd: root,
  env: process.env,
  stdin: 'inherit',
  stdout: 'inherit',
  stderr: 'inherit',
})

process.on('SIGINT', () => agent.kill())
process.on('SIGTERM', () => agent.kill())
const status = await agent.exited
if (status !== 0) process.exit(status)
