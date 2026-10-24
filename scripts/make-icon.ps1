#requires -Version 5.1
<#
.SYNOPSIS
    Generates assets/dict-popup.ico (16/32/48 BMP + 256 PNG frames) and
    assets/icon-preview-256.png for dict-popup.

.DESCRIPTION
    Artwork is drawn programmatically with System.Drawing: a rounded-square
    indigo tile carrying a bold white glyph (the character "wen", U+6587).
    The glyph is built as a vector path from a bold CJK face, then scaled to
    a fixed ink box, so it stays centred and legible from 256 px down to
    16 px.

    Each frame is rendered at 4x and downsampled with HighQualityBicubic.
    16/32/48 are stored as uncompressed 32-bit BMP (universally supported by
    the shell); 256 is stored PNG-compressed, as the ICO format intends.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\make-icon.ps1
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\make-icon.ps1 -OutDir .release-check\iconprobe -DebugSheet
#>
param(
    [string]$OutDir = '',
    [switch]$DebugSheet
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.Drawing

# NB: $PSScriptRoot is empty inside parameter defaults on Windows PowerShell
# 5.1, so resolve the default output directory here.
if (-not $OutDir) {
    $OutDir = Join-Path $PSScriptRoot '..\assets'
}

$Sizes = @(16, 32, 48, 256)

# Palette: deep indigo tile, white glyph.
$TileTop = [System.Drawing.Color]::FromArgb(255, 0x6A, 0x5B, 0xF0)
$TileBottom = [System.Drawing.Color]::FromArgb(255, 0x2B, 0x23, 0x6E)

# U+6587, the character "wen" (text / writing).
$Glyph = [string][char]0x6587

function Get-GlyphFontFamily {
    foreach ($name in @('Microsoft YaHei', 'SimHei', 'SimSun')) {
        try { return (New-Object System.Drawing.FontFamily $name) } catch { }
    }
    return [System.Drawing.FontFamily]::GenericSansSerif
}

function New-RoundedRectPath {
    param([single]$Size, [single]$Radius)
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $Radius * 2
    $p.AddArc(0, 0, $d, $d, 180, 90)
    $p.AddArc(($Size - $d), 0, $d, $d, 270, 90)
    $p.AddArc(($Size - $d), ($Size - $d), $d, $d, 0, 90)
    $p.AddArc(0, ($Size - $d), $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

function New-IconBitmap {
    param([int]$Size, [int]$Scale)

    $canvas = $Size * $Scale
    $bmp = New-Object System.Drawing.Bitmap $canvas, $canvas, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    try {
        $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
        $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $g.Clear([System.Drawing.Color]::Transparent)

        $rect = New-Object System.Drawing.RectangleF 0, 0, $canvas, $canvas
        $path = New-RoundedRectPath -Size $canvas -Radius ($canvas * 0.22)
        $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush $rect, $TileTop, $TileBottom, ([single]90.0)
        try { $g.FillPath($brush, $path) } finally { $brush.Dispose(); $path.Dispose() }

        # Build the glyph as a vector path, then scale/centre it so its ink
        # fills a fixed fraction of the tile. Doing this geometrically (rather
        # than DrawString) keeps the mark centred regardless of font metrics
        # and lets the same proportions survive downsampling to 16 px.
        $family = Get-GlyphFontFamily
        $glyphPath = New-Object System.Drawing.Drawing2D.GraphicsPath
        $transform = $null
        try {
            $fmt = [System.Drawing.StringFormat]::GenericTypographic
            $glyphPath.AddString($Glyph, $family, [int][System.Drawing.FontStyle]::Bold, ([single]($canvas * 0.9)), (New-Object System.Drawing.PointF 0, 0), $fmt)
            $bounds = $glyphPath.GetBounds()
            $targetW = $canvas * 0.58
            $targetH = $canvas * 0.70
            # NB: not "$scale" -- PowerShell variable names are case-insensitive
            # and would clobber the $Scale parameter.
            $glyphScale = [math]::Min(($targetW / $bounds.Width), ($targetH / $bounds.Height))
            # Build the matrix explicitly: scale, then centre the *ink* box.
            # (Chained Matrix.Translate/Scale calls compose in a surprising
            # order in GDI+, which shifts off-centre marks.)
            $dx = (($canvas - $bounds.Width * $glyphScale) / 2) - $bounds.X * $glyphScale
            $dy = (($canvas - $bounds.Height * $glyphScale) / 2) - $bounds.Y * $glyphScale
            $transform = New-Object System.Drawing.Drawing2D.Matrix ([single]$glyphScale), 0, 0, ([single]$glyphScale), ([single]$dx), ([single]$dy)
            $glyphPath.Transform($transform)
            $g.FillPath([System.Drawing.Brushes]::White, $glyphPath)
        } finally {
            $glyphPath.Dispose()
            if ($null -ne $transform) { $transform.Dispose() }
            $family.Dispose()
        }
    } finally { $g.Dispose() }

    if ($Scale -eq 1) { return $bmp }

    $out = New-Object System.Drawing.Bitmap $Size, $Size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g2 = [System.Drawing.Graphics]::FromImage($out)
    try {
        $g2.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $g2.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $g2.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
        $dest = New-Object System.Drawing.Rectangle 0, 0, $Size, $Size
        $g2.DrawImage($bmp, $dest, 0, 0, $bmp.Width, $bmp.Height, [System.Drawing.GraphicsUnit]::Pixel)
    } finally { $g2.Dispose(); $bmp.Dispose() }
    return $out
}

function Get-PngBytes {
    param([System.Drawing.Bitmap]$Bitmap)
    $ms = New-Object System.IO.MemoryStream
    try {
        $Bitmap.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
        $bytes = $ms.ToArray()
        return ,$bytes
    } finally { $ms.Dispose() }
}

# 32-bit BGRA DIB as the ICO format wants it: BITMAPINFOHEADER with doubled
# height, bottom-up XOR pixels, then a zeroed 1-bpp AND mask.
function Get-DibBytes {
    param([System.Drawing.Bitmap]$Bitmap)
    $w = $Bitmap.Width
    $h = $Bitmap.Height
    $stride = $w * 4
    $rect = New-Object System.Drawing.Rectangle 0, 0, $w, $h
    $data = $Bitmap.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    try {
        $raw = New-Object 'byte[]' ($stride * $h)
        [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $raw, 0, $raw.Length)
        $rowBytes = $data.Stride
    } finally { $Bitmap.UnlockBits($data) }

    $andStride = [int]([math]::Floor(($w + 31) / 32) * 4)
    $andSize = $andStride * $h
    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter $ms
    try {
        $bw.Write([uint32]40)
        $bw.Write([int32]$w)
        $bw.Write([int32]($h * 2))
        $bw.Write([uint16]1)
        $bw.Write([uint16]32)
        $bw.Write([uint32]0)
        $bw.Write([uint32]($stride * $h))
        $bw.Write([int32]0)
        $bw.Write([int32]0)
        $bw.Write([uint32]0)
        $bw.Write([uint32]0)
        for ($y = $h - 1; $y -ge 0; $y--) {
            $bw.Write($raw, $y * $rowBytes, $stride)
        }
        $bw.Write((New-Object 'byte[]' $andSize))
        $bw.Flush()
        $bytes = $ms.ToArray()
        return ,$bytes
    } finally { $bw.Dispose(); $ms.Dispose() }
}

function Write-IcoFile {
    param([string]$Path, [object[]]$Frames)
    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter $ms
    try {
        $bw.Write([uint16]0)
        $bw.Write([uint16]1)
        $bw.Write([uint16]$Frames.Count)
        $offset = 6 + 16 * $Frames.Count
        foreach ($f in $Frames) {
            $dim = if ($f.Size -ge 256) { 0 } else { $f.Size }
            $bw.Write([byte]$dim)
            $bw.Write([byte]$dim)
            $bw.Write([byte]0)
            $bw.Write([byte]0)
            $bw.Write([uint16]1)
            $bw.Write([uint16]32)
            $bw.Write([uint32]$f.Bytes.Length)
            $bw.Write([uint32]$offset)
            $offset += $f.Bytes.Length
        }
        foreach ($f in $Frames) { $bw.Write($f.Bytes) }
        $bw.Flush()
        [System.IO.File]::WriteAllBytes($Path, $ms.ToArray())
    } finally { $bw.Dispose(); $ms.Dispose() }
}

if ([System.IO.Path]::IsPathRooted($OutDir)) {
    $outPath = [System.IO.Path]::GetFullPath($OutDir)
} else {
    $outPath = [System.IO.Path]::GetFullPath((Join-Path (Get-Location).Path $OutDir))
}
New-Item -ItemType Directory -Force -Path $outPath | Out-Null
$icoPath = Join-Path $outPath 'dict-popup.ico'
$previewPath = Join-Path $outPath 'icon-preview-256.png'

$frames = @()
$pngs = @{}
foreach ($size in $Sizes) {
    $bitmap = New-IconBitmap -Size $size -Scale 8
    try {
        if ($size -ge 256) {
            $frames += @{ Size = $size; Bytes = (Get-PngBytes -Bitmap $bitmap) }
        } else {
            $frames += @{ Size = $size; Bytes = (Get-DibBytes -Bitmap $bitmap) }
        }
        $pngs[$size] = Get-PngBytes -Bitmap $bitmap
    } finally { $bitmap.Dispose() }
}

Write-IcoFile -Path $icoPath -Frames $frames
[System.IO.File]::WriteAllBytes($previewPath, $pngs[256])

if ($DebugSheet) {
    # 16/32/48 scaled 8x nearest-neighbour, side by side, to judge legibility.
    $cell = 256
    $sheet = New-Object System.Drawing.Bitmap ($cell * 3), $cell, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($sheet)
    try {
        $g.Clear([System.Drawing.Color]::FromArgb(255, 245, 245, 245))
        $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
        $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
        $labelFont = New-Object System.Drawing.Font 'Segoe UI', 11
        try {
            for ($i = 0; $i -lt 3; $i++) {
                $sz = @(16, 32, 48)[$i]
                $bmp = New-IconBitmap -Size $sz -Scale 8
                try {
                    $g.DrawImage($bmp, ($i * $cell + 32), 32, ($cell - 64), ($cell - 64))
                } finally { $bmp.Dispose() }
                $g.DrawString("$sz px (8x)", $labelFont, [System.Drawing.Brushes]::Black, ($i * $cell + 32), 8)
            }
        } finally { $labelFont.Dispose() }
    } finally { $g.Dispose() }
    $debugPath = Join-Path $outPath 'icon-debug-sizes.png'
    $sheet.Save($debugPath, [System.Drawing.Imaging.ImageFormat]::Png)
    $sheet.Dispose()
    Write-Host "debug sheet: $debugPath"
}

$ico = Get-Item $icoPath
$preview = Get-Item $previewPath
Write-Host ("ico     : {0}  ({1} bytes)" -f $ico.FullName, $ico.Length)
Write-Host ("preview : {0}  ({1} bytes)" -f $preview.FullName, $preview.Length)
foreach ($f in $frames) { Write-Host ("  frame {0,3}px : {1,6} bytes" -f $f.Size, $f.Bytes.Length) }
