# spotify-lyrics-proxy

A simple proxy between the private Spotify API (Musixmatch) and your application.

## Usage

### Configuration

All configuration is done via the `config.toml` file.

```toml
cookies = [
    "1234567890abcdef",
    "abcdef1234567890",
]
# Optional - List of API keys to use (if not provided, the proxy will be open to anyone)
api_keys = ["smash-your-keyboard-here"]
# Optional - The number of requests allowed per second (if not provided no ratelimiting will be done)
ratelimit = 60
# Optional (default: 3000) - The port to listen on
port = 3000

```