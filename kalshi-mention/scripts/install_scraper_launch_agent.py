"""Install or remove a daily macOS launchd job for the corpus refresh."""

from __future__ import annotations

import argparse
import json
import os
import plistlib
import subprocess
import sys
from pathlib import Path


LABEL = "com.polytaoli.kalshi-mention-corpus-refresh"
EVALUATION_LABEL = "com.polytaoli.directional-paper-evaluation"
VENDOR_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = VENDOR_ROOT.parent
DEFAULT_DATA_DIR = REPO_ROOT / "data" / "kalshi_mention"


def launch_agent(hour: int, minute: int = 0, evaluation: bool = False) -> dict:
    if not 0 <= hour <= 23:
        raise ValueError("hour must be between 0 and 23")
    if not 0 <= minute <= 59:
        raise ValueError("minute must be between 0 and 59")
    python = VENDOR_ROOT / ".venv" / "bin" / "python"
    if not python.is_file():
        raise FileNotFoundError(f"project virtualenv interpreter not found: {python}")

    data_dir = Path(os.environ.get("KALSHI_MENTION_DATA_DIR", DEFAULT_DATA_DIR)).expanduser()
    log_dir = data_dir / "logs"
    environment = {
        "KALSHI_MENTION_DATA_DIR": str(data_dir),
        "KALSHI_MENTION_MAX_CORPUS_AGE_DAYS": os.environ.get(
            "KALSHI_MENTION_MAX_CORPUS_AGE_DAYS", "14"
        ),
    }
    if "KALSHI_MENTION_CORPUS" in os.environ:
        environment["KALSHI_MENTION_CORPUS"] = os.environ["KALSHI_MENTION_CORPUS"]

    label = EVALUATION_LABEL if evaluation else LABEL
    log_prefix = "directional-evaluation" if evaluation else "corpus-refresh"
    arguments = (
        [str(python), "-c", "from kalshi_mention.directional_evaluation import run_once; run_once()"]
        if evaluation else [str(python), "-m", "kalshi_mention.refresh_corpus"]
    )
    return {
        "Label": label,
        "ProgramArguments": arguments,
        "WorkingDirectory": str(VENDOR_ROOT),
        "EnvironmentVariables": environment,
        "StartCalendarInterval": {"Hour": hour, "Minute": minute},
        "StandardOutPath": str(log_dir / f"{log_prefix}.log"),
        "StandardErrorPath": str(log_dir / f"{log_prefix}-error.log"),
    }


def install_agent(definition: dict) -> Path:
    launch_agents = Path.home() / "Library" / "LaunchAgents"
    launch_agents.mkdir(parents=True, exist_ok=True)
    agent_path = launch_agents / f"{definition['Label']}.plist"
    payload = plistlib.dumps(definition, fmt=plistlib.FMT_XML, sort_keys=True)

    if agent_path.exists():
        try:
            existing = plistlib.loads(agent_path.read_bytes())
        except Exception as exc:
            raise RuntimeError(f"refusing to replace unreadable launch agent: {agent_path}") from exc
        if existing != definition:
            raise RuntimeError(
                f"launch agent already exists with different settings: {agent_path}; "
                "review and remove it before reinstalling"
            )
    else:
        data_dir = Path(definition["EnvironmentVariables"]["KALSHI_MENTION_DATA_DIR"])
        (data_dir / "logs").mkdir(parents=True, exist_ok=True)
        agent_path.write_bytes(payload)

    domain = f"gui/{os.getuid()}"
    subprocess.run(["launchctl", "bootout", domain, str(agent_path)], check=False,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    subprocess.run(["launchctl", "bootstrap", domain, str(agent_path)], check=True)
    return agent_path


def install(hour: int) -> list[Path]:
    return [
        install_agent(launch_agent(hour, 0)),
        install_agent(launch_agent(hour, 15, evaluation=True)),
    ]


def uninstall() -> list[Path]:
    removed = []
    for label in (LABEL, EVALUATION_LABEL):
        agent_path = Path.home() / "Library" / "LaunchAgents" / f"{label}.plist"
        if agent_path.exists():
            subprocess.run(["launchctl", "bootout", f"gui/{os.getuid()}", str(agent_path)],check=False)
            agent_path.unlink()
        removed.append(agent_path)
    return removed


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hour", type=int, default=6, help="local hour for the daily run (default: 6)")
    parser.add_argument("--print", action="store_true", help="print both LaunchAgent definitions without installing")
    parser.add_argument("--uninstall", action="store_true", help="remove the installed LaunchAgent")
    args = parser.parse_args()

    if args.uninstall:
        print(f"Removed LaunchAgents: {uninstall()}")
    elif args.print:
        definitions = [launch_agent(args.hour), launch_agent(args.hour, 15, evaluation=True)]
        sys.stdout.write(json.dumps(definitions, indent=2) + "\n")
    else:
        print(f"Installed daily corpus refresh and paper evaluator: {install(args.hour)}")


if __name__ == "__main__":
    main()