"""Set up and administer a private Prestman server from its Docker host."""

import argparse
import base64
import getpass
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import tempfile
import time

DIRECTORY = Path(__file__).resolve().parent
COMPOSE = DIRECTORY / "compose.yml"
HTTPS_COMPOSE = DIRECTORY / "compose.https.yml"


def signed_key(secret, role, lifetime):
    def encode(value):
        payload = json.dumps(value, separators=(",", ":")).encode()
        return base64.urlsafe_b64encode(payload).decode().rstrip("=")

    now = int(time.time())
    header = encode({"alg": "HS256", "typ": "JWT"})
    claims = encode({"iss": "supabase", "role": role, "iat": now, "exp": now + lifetime})
    message = f"{header}.{claims}"
    signature = hmac.new(secret.encode(), message.encode(), hashlib.sha256).digest()
    return f"{message}.{base64.urlsafe_b64encode(signature).decode().rstrip('=')}"


def initialize(env_file, port, domain=None):
    secret = secrets.token_hex(32)
    config = {
        "PRESTMAN_PORT": str(port),
        "POSTGRES_PASSWORD": secrets.token_hex(32),
        "AUTH_DB_PASSWORD": secrets.token_hex(32),
        "REST_DB_PASSWORD": secrets.token_hex(32),
        "JWT_SECRET": secret,
        "ANON_KEY": signed_key(secret, "anon", 5 * 365 * 86400),
    }
    if domain:
        config["PRESTMAN_DOMAIN"] = domain
    with os.fdopen(os.open(env_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as file:
        for key, value in config.items():
            file.write(f"{key}={value}\n")


def read_config(env_file):
    config = dict(line.split("=", 1) for line in env_file.read_text().splitlines()
                  if line and not line.startswith("#"))
    if config.get("PRESTMAN_DOMAIN"):
        config["PRESTMAN_DOMAIN"] = parse_domain(config["PRESTMAN_DOMAIN"])
    return config


def parse_domain(value):
    domain = value.rstrip(".").lower()
    suffix = r"(?:[a-z0-9-]{0,61}[a-z0-9])?"
    if len(domain) > 253 or not re.fullmatch(rf"(?:[a-z0-9]{suffix}\.)+[a-z]{suffix}", domain):
        raise ValueError("Enter a domain such as cloud.example.com, without a URL or port.")
    return domain


def prepare_server(env_file, port, domain):
    try:
        initialize(env_file, port or 8800, domain)
    except FileExistsError:
        pass
    config = read_config(env_file)
    updates = {}
    if port is not None:
        updates["PRESTMAN_PORT"] = str(port)
    if domain is not None:
        updates["PRESTMAN_DOMAIN"] = domain
    if any(config.get(key) != value for key, value in updates.items()):
        config.update(updates)
        file = tempfile.NamedTemporaryFile(mode="w", dir=env_file.parent, delete=False)
        temporary = Path(file.name)
        try:
            with file:
                for key, value in config.items():
                    file.write(f"{key}={value}\n")
            temporary.replace(env_file)
        finally:
            temporary.unlink(missing_ok=True)
    return config


def run_compose(env_file, config, arguments, *, project=None, **options):
    command = ["docker", "compose", "--env-file", str(env_file), "-f", str(COMPOSE)]
    if config.get("PRESTMAN_DOMAIN"):
        command.extend(["-f", str(HTTPS_COMPOSE)])
    if project:
        command.extend(["--project-name", project])
    return subprocess.run(command + arguments, env={**os.environ, **config}, check=True, **options)


def client_origin(config):
    if domain := config.get("PRESTMAN_DOMAIN"):
        return f"https://{domain}"
    return f"http://127.0.0.1:{config['PRESTMAN_PORT']}"


def print_client_info(config):
    print(f"Server URL: {client_origin(config)}")
    print(f"Public API key: {config['ANON_KEY']}")


def create_user(env_file, email, password, project=None):
    config = read_config(env_file)
    token = signed_key(config["JWT_SECRET"], "service_role", 300)
    payload = json.dumps({"email": email, "password": password, "email_confirm": True})

    arguments = ["exec", "-T", "auth", "sh", "-ec", """
IFS= read -r token
IFS= read -r payload
wget -q -O - --header "Authorization: Bearer $token" \
    --header "Content-Type: application/json" --post-data "$payload" \
    http://127.0.0.1:9999/admin/users
"""]
    try:
        run_compose(
            env_file, config, arguments, project=project,
            input=f"{token}\n{payload}\n", text=True, stdout=subprocess.DEVNULL,
        )
    except subprocess.CalledProcessError as error:
        raise ValueError("Could not create account. Check that Auth is running and the address is unused.") from error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env-file", type=Path, default=DIRECTORY / ".env")
    subcommands = parser.add_subparsers(dest="command", required=True)
    initialize_parser = subcommands.add_parser("init", help="Generate local server secrets once")
    initialize_parser.add_argument("--port", type=int, default=8800)
    up_parser = subcommands.add_parser("up", help="Set up and start the server")
    up_parser.add_argument("--port", type=int, help="Loopback HTTP port (default: 8800)")
    up_parser.add_argument("--domain", help="Public DNS name for automatic HTTPS")
    subcommands.add_parser("status", help="Show the status of the server containers")
    subcommands.add_parser("down", help="Stop the server and keep its data")
    subcommands.add_parser("client-info", help="Print the URL and public API key for the app")
    subcommands.add_parser("client-env", help="Print only the URL and the public client key")
    user_parser = subcommands.add_parser("create-user", help="Create a confirmed local account")
    user_parser.add_argument("email")

    args = parser.parse_args()
    try:
        if hasattr(args, "port") and args.port is not None and not 1024 <= args.port <= 65535:
            parser.error("The port must be between 1024 and 65535.")
        env_file = args.env_file.resolve()
        if args.command == "init":
            initialize(env_file, args.port)
            print(f"Created {env_file}")
        elif args.command == "up":
            domain = parse_domain(args.domain) if args.domain is not None else None
            config = prepare_server(env_file, args.port, domain)
            run_compose(env_file, config, ["up", "-d", "--wait"])
            print_client_info(config)
        elif args.command in {"status", "down"}:
            action = ["ps", "-a"] if args.command == "status" else ["down"]
            run_compose(env_file, read_config(env_file), action)
        elif args.command in {"client-info", "client-env"}:
            config = read_config(env_file)
            if args.command == "client-info":
                print_client_info(config)
            else:
                print(f"export PRESTMAN_CLOUD_URL={shlex.quote(client_origin(config))}")
                print(f"export PRESTMAN_CLOUD_KEY={shlex.quote(config['ANON_KEY'])}")
        elif args.command == "create-user":
            password = getpass.getpass("Initial password: ")
            if len(password) < 12 or len(password.encode()) > 72:
                parser.error("Use a password at least 12 characters long and at most 72 UTF-8 bytes.")
            if password != getpass.getpass("Repeat password: "):
                parser.error("Passwords do not match.")
            create_user(env_file, args.email, password)
            print(f"Created confirmed account for {args.email}.")
    except subprocess.CalledProcessError as error:
        parser.exit(
            error.returncode, "Docker Compose could not complete the operation. See the output above.\n"
        )
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
