#requires -Version 5.1
<#
.SYNOPSIS
    Sign the release binary with a local code-signing certificate.

.DESCRIPTION
    Creates a self-signed code-signing certificate (if one is not already in the
    current user's store), installs it into the current user's Root and
    TrustedPublisher stores so Windows treats the signature as trusted, then
    signs the executable.

    A self-signed certificate removes the "unknown publisher" warning **on this
    machine only**. Other people's machines will still warn until the binary is
    signed with a certificate from a public CA (OV/EV code signing) - see
    README "代码签名".

    powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts\sign-release.ps1
#>
[CmdletBinding()]
param(
    [string]$Exe = 'F:\project\translation\dist\dict-popup.exe',
    [string]$Subject = 'CN=XuJake, O=XuJake, C=CN',
    [int]$ValidYears = 5
)

$ErrorActionPreference = 'Stop'
$codeSigningOid = '1.3.6.1.5.5.7.3.3'

if (-not (Test-Path -LiteralPath $Exe)) {
    throw "not found: $Exe (run build.bat first)"
}

$cert = Get-ChildItem Cert:\CurrentUser\My -ErrorAction SilentlyContinue |
    Where-Object { $_.Subject -eq $Subject -and $_.EnhancedKeyUsageList.ObjectId -contains $codeSigningOid } |
    Sort-Object NotAfter -Descending |
    Select-Object -First 1

if (-not $cert) {
    Write-Host "== creating self-signed code-signing certificate"
    $cert = New-SelfSignedCertificate `
        -Type CodeSigningCert `
        -Subject $Subject `
        -KeyUsage DigitalSignature `
        -KeyAlgorithm RSA `
        -KeyLength 3072 `
        -HashAlgorithm SHA256 `
        -FriendlyName 'dict-popup code signing' `
        -CertStoreLocation Cert:\CurrentUser\My `
        -NotAfter (Get-Date).AddYears($ValidYears)
} else {
    Write-Host "== using existing certificate $($cert.Thumbprint)"
}

# Export the public part so the user can import it elsewhere if needed.
$assetDir = Join-Path (Split-Path -Parent (Split-Path -Parent $PSCommandPath)) 'assets\signing'
New-Item -ItemType Directory -Force -Path $assetDir | Out-Null
$cerPath = Join-Path $assetDir 'dict-popup-codesign.cer'
Export-Certificate -Cert $cert -FilePath $cerPath -Force | Out-Null
Write-Host "== public certificate: $cerPath"

# Trust it locally. X509Store.Add does not raise the interactive warning that
# certutil/Explorer show, so this can run unattended.
foreach ($storeName in @('TrustedPublisher', 'Root')) {
    $store = New-Object System.Security.Cryptography.X509Certificates.X509Store($storeName, 'CurrentUser')
    $store.Open([System.Security.Cryptography.X509Certificates.OpenFlags]::ReadWrite)
    try {
        $store.Add($cert)
        Write-Host "== added to CurrentUser\$storeName"
    } catch {
        Write-Warning "could not add to CurrentUser\${storeName}: $($_.Exception.Message)"
    } finally {
        $store.Close()
    }
}

Write-Host "== signing $Exe"
$signature = Set-AuthenticodeSignature -FilePath $Exe -Certificate $cert -HashAlgorithm SHA256
Write-Host "== signature status: $($signature.Status)"

Get-AuthenticodeSignature -FilePath $Exe |
    Select-Object Status, StatusMessage, @{n = 'Signer'; e = { $_.SignerCertificate.Subject } }, @{n = 'Thumbprint'; e = { $_.SignerCertificate.Thumbprint } } |
    Format-List
