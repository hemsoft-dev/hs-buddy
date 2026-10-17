"""Temporary file-only investigation for mission837. Never launches the app."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--image', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
image = args.image.resolve(strict=True)
assert hashlib.sha256(image.read_bytes()).hexdigest() == 'b87fdc4beffb55260a92d3f6270121bff99fd045ce2e55199b95e553d59a8de8'
output = args.output.resolve()
assert output.is_relative_to(Path(os.environ['RUNNER_TEMP']).resolve()) and not output.exists()
output.mkdir()
source = Path(__file__).with_name('macos-ocr-words').joinpath('main.swift').read_text(encoding='utf-8')
(output / 'main.swift').write_text(source, encoding='utf-8')


def run(name, swift):
    path = output / (name + '.swift')
    path.write_text(swift, encoding='utf-8')
    binary = output / name
    compiled = subprocess.run(['swiftc', '-module-cache-path', str(output / 'module-cache'), str(path), '-o', str(binary)], capture_output=True, timeout=180)
    (output / (name + '-compile.stdout')).write_bytes(compiled.stdout)
    (output / (name + '-compile.stderr')).write_bytes(compiled.stderr)
    assert compiled.returncode == 0, 'File-only OCR probe compilation failed'
    result = subprocess.run([str(binary), str(image)], capture_output=True, timeout=60)
    (output / (name + '.json')).write_bytes(result.stdout)
    (output / (name + '.stderr')).write_bytes(result.stderr)
    return result.returncode


original_exit = run('original', source)
diagnostic_exit = None
if original_exit != 0:
    diagnostic = source.replace('do {\n    guard CommandLine', 'var stage = "image-decode"\ndo {\n    guard CommandLine', 1)
    diagnostic = diagnostic.replace('    try VNImageRequestHandler', '    stage = "recognize"\n    try VNImageRequestHandler', 1)
    diagnostic = diagnostic.replace('            guard let bounds = try candidate.boundingBox', '            stage = "token-box"\n            guard let bounds = try candidate.boundingBox', 1)
    diagnostic = diagnostic.replace('            let box = bounds.boundingBox', '            stage = "box-limits"\n            let box = bounds.boundingBox', 1)
    diagnostic = diagnostic.replace('else { throw OcrFailure.invalidBounds }', 'else {\n                FileHandle.standardError.write(Data("Invalid box minX=\\(box.minX) minY=\\(box.minY) maxX=\\(box.maxX) maxY=\\(box.maxY) width=\\(box.width) height=\\(box.height)\\n".utf8))\n                throw OcrFailure.invalidBounds\n            }', 1)
    old = '    FileHandle.standardError.write(Data("Native image OCR failed.\\n".utf8))'
    new = '    let failure = error as NSError\n    let kind = error is OcrFailure ? String(describing: error) : "framework-error"\n    FileHandle.standardError.write(Data("Native image OCR failed at \\(stage): \\(kind), code=\\(failure.code).\\n".utf8))'
    assert old in diagnostic
    diagnostic = diagnostic.replace(old, new)
    diagnostic_exit = run('diagnostic', diagnostic)

report = dict(purpose='diagnosis-only', appQualified=False, imageSha256=hashlib.sha256(image.read_bytes()).hexdigest(),
    helperSha256=hashlib.sha256(source.encode()).hexdigest(), checkoutHead=os.environ.get('GITHUB_SHA'),
    host=subprocess.check_output(['sw_vers'], text=True), architecture=subprocess.check_output(['uname', '-m'], text=True).strip(),
    originalExit=original_exit, diagnosticExit=diagnostic_exit)
(output / 'result.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
print(json.dumps(report))
if diagnostic_exit is not None:
    print((output / 'diagnostic.stderr').read_text(encoding='utf-8'))
