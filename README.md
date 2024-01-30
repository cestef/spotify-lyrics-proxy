# spotify-lyrics-proxy

A simple proxy between the private Spotify API (Musixmatch) and your application.

## Usage

### Configuration

All configuration is done via the `config.toml` file.

```toml
# Optional
api_key = "smash-your-keyboard-here"
cookies = [
    "1234567890abcdef",
    "abcdef1234567890",
]
ratelimit = 10
ratelimit_reset = 1
# Optional
port = 3000

```