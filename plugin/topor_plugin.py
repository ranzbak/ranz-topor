"""Topor KiCad plugin entry point (M0: engine version handshake only; M4 adds the IPC work)."""
import sys

# Rewritten by tools/build.sh to the engine version this plugin was shipped with.
SHIPPED_VERSION = "0.0.1"


def main() -> int:
    import topor_engine

    got = topor_engine.version()
    if got != SHIPPED_VERSION:
        print(
            f"topor: engine version mismatch: installed {got}, plugin shipped with "
            f"{SHIPPED_VERSION}. Restart the PCB Editor or use Recreate Plugin Environment.",
            file=sys.stderr,
        )
        return 1
    print(f"topor engine {got} ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
