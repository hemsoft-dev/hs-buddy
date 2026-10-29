export function verifyBinaryArchitecture(bytes, target) {
  if (bytes.length < 64) throw new Error('Native binary is truncated.')
  let matches = false
  if (target === 'x86_64-pc-windows-msvc' && bytes.toString('ascii', 0, 2) === 'MZ') {
    const header = bytes.readUInt32LE(0x3c)
    matches =
      header + 6 <= bytes.length &&
      bytes.readUInt32LE(header) === 0x4550 &&
      bytes.readUInt16LE(header + 4) === 0x8664
  } else if (target === 'x86_64-unknown-linux-gnu') {
    matches =
      bytes.toString('hex', 0, 4) === '7f454c46' &&
      bytes[4] === 2 &&
      bytes[5] === 1 &&
      bytes.readUInt16LE(18) === 62
  } else if (target === 'x86_64-apple-darwin' || target === 'aarch64-apple-darwin') {
    const cpu = target === 'x86_64-apple-darwin' ? 0x01000007 : 0x0100000c
    matches = bytes.readUInt32LE(0) === 0xfeedfacf && bytes.readUInt32LE(4) === cpu
  }
  if (!matches) throw new Error(`Native binary does not match ${target}.`)
}

export function windowsImports(bytes) {
  verifyBinaryArchitecture(bytes, 'x86_64-pc-windows-msvc')
  const pe = bytes.readUInt32LE(0x3c)
  if (pe + 24 > bytes.length) throw new Error('Truncated PE file header.')
  const optional = pe + 24
  const optionalSize = bytes.readUInt16LE(pe + 20)
  const sectionCount = bytes.readUInt16LE(pe + 6)
  const sections = optional + optionalSize
  if (
    optionalSize < 128 ||
    optional + optionalSize > bytes.length ||
    sections + sectionCount * 40 > bytes.length ||
    bytes.readUInt16LE(optional) !== 0x20b
  )
    throw new Error('Invalid PE64 headers.')
  const offset = (rva, length) => {
    for (let index = 0; index < sectionCount; index++) {
      const section = sections + index * 40
      const address = bytes.readUInt32LE(section + 12)
      const rawSize = bytes.readUInt32LE(section + 16)
      const start = bytes.readUInt32LE(section + 20)
      if (
        rva >= address &&
        rva - address + length <= rawSize &&
        start + rva - address + length <= bytes.length
      )
        return start + rva - address
    }
    throw new Error('PE import address is outside file sections.')
  }
  const directory = bytes.readUInt32LE(optional + 120)
  const directorySize = bytes.readUInt32LE(optional + 124)
  if (directory === 0 && directorySize === 0) return []
  const names = []
  for (let index = 0; index < Math.min(Math.floor(directorySize / 20), 512); index++) {
    const descriptor = offset(directory + index * 20, 20)
    if ([0, 4, 8, 12, 16].every(field => bytes.readUInt32LE(descriptor + field) === 0)) return names
    const nameRva = bytes.readUInt32LE(descriptor + 12)
    let name = ''
    for (let byte = 0; byte < 512; byte++) {
      const value = bytes[offset(nameRva + byte, 1)]
      if (value === 0) break
      if (value < 33 || value > 126) throw new Error('Invalid PE import name.')
      name += String.fromCharCode(value)
      if (byte === 511) throw new Error('Unterminated PE import name.')
    }
    if (!name) throw new Error('Empty PE import name.')
    names.push(name.toLowerCase())
  }
  throw new Error('Unterminated PE import table.')
}

const windowsSystemLibraries = new Set([
  'kernel32.dll',
  'advapi32.dll',
  'user32.dll',
  'gdi32.dll',
  'ole32.dll',
  'imm32.dll',
  'combase.dll',
  'dwmapi.dll',
  'd3dcompiler_47.dll',
  'icuuc.dll',
  'dcomp.dll',
  'shell32.dll',
  'comctl32.dll',
  'dxgi.dll',
  'd3d11.dll',
  'dwrite.dll',
  'winmm.dll',
  // Windows' audio device API; rodio 0.23's WASAPI output imports it directly.
  'mmdevapi.dll',
  'ntdll.dll',
  'uiautomationcore.dll',
  'oleaut32.dll',
  'ws2_32.dll',
  'bcryptprimitives.dll',
  'gdiplus.dll',
  'bcrypt.dll',
  'userenv.dll',
  'version.dll',
  'propsys.dll',
  'shlwapi.dll',
  'secur32.dll',
  'crypt32.dll',
  'rpcrt4.dll',
  'ntmarta.dll',
  'winhttp.dll',
  'iphlpapi.dll',
  'normaliz.dll',
])

export function verifyStandaloneWindows(bytes) {
  const imports = windowsImports(bytes)
  const external = imports.filter(
    name => !windowsSystemLibraries.has(name) && !/^(?:api|ext)-ms-win-.+\.dll$/.test(name)
  )
  if (external.length)
    throw new Error(
      `Native Windows package has undeclared runtime DLLs: ${external.join(', ')}. Build MSVC with crt-static or explicitly bundle dependencies.`
    )
  return imports
}

export function verifyStandaloneMac(output) {
  const lines = output.trim().split(/\r?\n/).slice(1)
  if (!lines.length || lines.some(line => !line.includes(' (compatibility version ')))
    throw new Error('Could not inspect macOS runtime libraries.')
  const libraries = lines.map(line => line.trim().split(' (compatibility version ')[0])
  const external = libraries.filter(
    path => !path.startsWith('/System/Library/') && !path.startsWith('/usr/lib/')
  )
  if (external.length)
    throw new Error(`Native Mac package has unbundled runtime libraries: ${external.join(', ')}.`)
  return libraries
}
