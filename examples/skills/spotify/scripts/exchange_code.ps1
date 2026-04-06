param(
    [Parameter(Mandatory=$true)]
    [string]$Code
)

$ErrorActionPreference = "Stop"

$ClientId = $env:SPOTIFY_CLIENT_ID
$ClientSecret = $env:SPOTIFY_CLIENT_SECRET

if (-not $ClientId) {
    Write-Output '{"error": "SPOTIFY_CLIENT_ID is not set."}'
    exit 1
}

if (-not $ClientSecret) {
    Write-Output '{"error": "SPOTIFY_CLIENT_SECRET is not set."}'
    exit 1
}

$RedirectUri = "http://127.0.0.1:8888/callback"

# Exchange the authorization code for tokens
$TokenBody = @{
    grant_type    = "authorization_code"
    code          = $Code
    redirect_uri  = $RedirectUri
    client_id     = $ClientId
    client_secret = $ClientSecret
}

try {
    $TokenResponse = Invoke-RestMethod -Uri "https://accounts.spotify.com/api/token" `
        -Method Post `
        -Body $TokenBody `
        -ContentType "application/x-www-form-urlencoded"
} catch {
    $ErrorBody = $_.ErrorDetails.Message
    if (-not $ErrorBody) { $ErrorBody = $_.Exception.Message }
    Write-Output "{\"error\": \"Token exchange failed: $ErrorBody\"}"
    exit 1
}

$AccessToken = $TokenResponse.access_token
$RefreshToken = $TokenResponse.refresh_token

if (-not $AccessToken -or -not $RefreshToken) {
    Write-Output '{"error": "Token response missing access_token or refresh_token."}'
    exit 1
}

Write-Output "Authorization successful!"
Write-Output ""
Write-Output "ACCESS_TOKEN=$AccessToken"
Write-Output "REFRESH_TOKEN=$RefreshToken"
Write-Output "EXPIRES_IN=$($TokenResponse.expires_in)"
Write-Output ""
Write-Output "Save the REFRESH_TOKEN in your skill configuration."
Write-Output "It does not expire and can be used to get new access tokens indefinitely."
