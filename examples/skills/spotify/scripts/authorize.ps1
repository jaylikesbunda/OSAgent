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

$Port = 8888
$RedirectUri = "http://127.0.0.1:$Port/callback"
$Scopes = "user-modify-playback-state user-read-playback-state user-read-currently-playing"

$AuthUrl = "https://accounts.spotify.com/authorize" +
    "?client_id=$([System.Uri]::EscapeDataString($ClientId))" +
    "&response_type=code" +
    "&redirect_uri=$([System.Uri]::EscapeDataString($RedirectUri))" +
    "&scope=$([System.Uri]::EscapeDataString($Scopes))"

$Endpoint = [System.Net.IPEndPoint]::new([System.Net.IPAddress]::Loopback, $Port)
$TcpListener = [System.Net.Sockets.TcpListener]::new($Endpoint)

try {
    $TcpListener.Start()
} catch {
    Write-Output ('{"error": "Failed to listen on port ' + $Port + ': ' + $_.Exception.Message + '"}')
    exit 1
}

Write-Output "Listening on port $Port..."
Write-Output "Opening browser for Spotify authorization..."
Start-Process $AuthUrl

$TcpListener.Server.ReceiveTimeout = 120000

try {
    $TcpClient = $TcpListener.AcceptTcpClient()
} catch {
    $TcpListener.Stop()
    Write-Output ('{"error": "Timed out waiting for authorization: ' + $_.Exception.Message + '"}')
    exit 1
}

$Stream = $TcpClient.GetStream()
$Stream.ReadTimeout = 10000
$Stream.WriteTimeout = 10000

$Buffer = [byte[]]::new(8192)
$Read = $Stream.Read($Buffer, 0, $Buffer.Length)
$RequestText = [System.Text.Encoding]::UTF8.GetString($Buffer, 0, $Read)

$CodeMatch = [regex]::Match($RequestText, '[?&]code=([^&\s]+)')
$AuthCode = if ($CodeMatch.Success) { $CodeMatch.Groups[1].Value } else { $null }

$ErrorMatch = [regex]::Match($RequestText, '[?&]error=([^&\s]+)')
$ErrorParam = if ($ErrorMatch.Success) { $ErrorMatch.Groups[1].Value } else { $null }

$Html = if ($AuthCode) {
    '<html><body style="font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;background:#121212;color:#fff"><div style="text-align:center"><h1 style="color:#1db954">Authorized!</h1><p>You can close this window.</p></div></body></html>'
} else {
    "<html><body style=`"font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;background:#121212;color:#fff`"><div style=`"text-align:center`"><h1 style=`"color:#e74c3c`">Failed</h1><p>$ErrorParam</p></div></body></html>"
}

$HtmlBytes = [System.Text.Encoding]::UTF8.GetBytes($Html)
$Response = "HTTP/1.1 200 OK`r`nContent-Type: text/html; charset=utf-8`r`nContent-Length: $($HtmlBytes.Length)`r`nConnection: close`r`n`r`n"
$ResponseBytes = [System.Text.Encoding]::ASCII.GetBytes($Response)
$Stream.Write($ResponseBytes, 0, $ResponseBytes.Length)
$Stream.Write($HtmlBytes, 0, $HtmlBytes.Length)
$Stream.Close()
$TcpClient.Close()
$TcpListener.Stop()

if ($ErrorParam -or -not $AuthCode) {
    Write-Output ('{"error": "Authorization denied or no code received."}')
    exit 1
}

Write-Output "Exchanging authorization code for tokens..."

$TokenBody = @{
    grant_type    = "authorization_code"
    code          = $AuthCode
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
    @{ error = "Token exchange failed: $ErrorBody" } | ConvertTo-Json -Compress
    exit 1
}

$AccessToken = $TokenResponse.access_token
$RefreshToken = $TokenResponse.refresh_token

if (-not $AccessToken -or -not $RefreshToken) {
    Write-Output '{"error": "Token response missing access_token or refresh_token."}'
    exit 1
}

Write-Output ""
Write-Output "Authorization successful!"
@{ refresh_token = $RefreshToken; access_token = $AccessToken } | ConvertTo-Json -Compress
