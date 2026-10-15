function Assert-NativeIcon {
    param([IntPtr] $Handle, [Parameter(Mandatory)] [string] $ExpectedIconPath)
    if ($Handle -eq [IntPtr]::Zero) { throw 'Registered Buddy window icon is missing' }
    $actual = [System.Drawing.Icon]::FromHandle($Handle)
    $expected = [System.Drawing.Icon]::new($ExpectedIconPath, $actual.Size)
    $actualPixels = $actual.ToBitmap()
    $expectedPixels = $expected.ToBitmap()
    try {
        for ($y = 0; $y -lt $actualPixels.Height; $y++) {
            for ($x = 0; $x -lt $actualPixels.Width; $x++) {
                $left = $actualPixels.GetPixel($x, $y)
                $right = $expectedPixels.GetPixel($x, $y)
                if ($left.A -eq 0 -and $right.A -eq 0) { continue }
                if ($left.ToArgb() -ne $right.ToArgb()) { throw 'Registered window icon differs from the bundled Buddy icon' }
            }
        }
        return @{ width = $actualPixels.Width; height = $actualPixels.Height; verified = $true }
    }
    finally { $actualPixels.Dispose(); $expectedPixels.Dispose(); $expected.Dispose() }
}
