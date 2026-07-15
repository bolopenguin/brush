from eyesplat.utils.decode_splat import decode_splats_from_bytes
import sys
from pathlib import Path

in_path = Path(sys.argv[1])
out_path = Path(sys.argv[2])

try:
    with open(in_path, "rb") as f:
        data = f.read()
except OSError as e:  # pragma: no cover
    raise OSError(f"Failed to read splat file '{in_path}': {e}") from e

nt = decode_splats_from_bytes(data)

out_path.parent.mkdir(parents=True, exist_ok=True)
nt.to_file(out_path)
