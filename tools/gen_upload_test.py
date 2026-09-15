"""Build a drag-and-drop upload fixture for the cmote disconnect hunt (PLAN §172 follow-up).

The point of the shapes below is the SSH packet, not the file. cmote uploads in 32 KiB chunks
(`CHUNK` in ssh/upload.rs), each chunk becoming one SFTP write inside one SSH packet, and the
session negotiates zlib compression. An INCOMPRESSIBLE 32 KiB chunk is therefore the interesting
one: deflate cannot shrink it, so its output lands within a few bytes of the reservation russh
makes for it, and that boundary is the suspect. A .zip of random bytes is the cheapest way to make
a file every one of whose chunks is incompressible.

So the fixture is deliberately weighted: most of the bytes are random and zipped, sizes straddle
32 KiB exactly, and a couple of files are large enough to produce hundreds of full incompressible
chunks in a row. Plain text files are included as the control - if those never fail and the zips
do, the compression path is implicated rather than the drop path.
"""

import os
import random
import shutil
import zipfile

# Fixture root: beside this script, so it has one home on every machine instead of a path that
# only existed on one. Its CONTENTS are gitignored - 10.4 MiB of deliberately incompressible zips
# has no business in history when this file regenerates them byte-identically.
kOutDir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "cmote-upload-test")
kTreeDir = "cmote-upload-tree"              # subfolder, for the folder-drop (tree walk) path
kChunk = 32 * 1024                          # cmote's upload CHUNK, the packet size that matters

# Sizes that straddle one upload chunk, so a write lands just short of, exactly on, and just past
# the boundary. The last two exist to produce many consecutive full chunks.
kStraddle = [
	("under", kChunk - 1),
	("exact", kChunk),
	("over", kChunk + 1),
	("two", kChunk * 2),
	("two-plus", kChunk * 2 + 7),
]
kBulk = [("bulk-1mib", 1 << 20), ("bulk-8mib", 8 << 20)]


def incompressible(inSize):
	"""Random bytes, which deflate cannot shrink - the whole point of the fixture."""
	return random.randbytes(inSize)


def write_zip(inPath, inSize):
	"""A .zip whose single member is random, so the archive itself is incompressible.

	DEFLATED rather than STORED so the file is a real compressed archive - what the user was
	actually uploading - and its bytes come out with no redundancy left for the SSH layer to find.
	"""
	with zipfile.ZipFile(inPath, "w", zipfile.ZIP_DEFLATED) as vZip:
		vZip.writestr("payload.bin", incompressible(inSize))


def write_raw(inPath, inSize):
	"""Raw random bytes with no container, to separate "is a zip" from "is incompressible"."""
	with open(inPath, "wb") as vFile:
		vFile.write(incompressible(inSize))


def write_text(inPath, inSize):
	"""Highly compressible filler - the control arm."""
	vLine = "the quick brown fox jumps over the lazy dog, and does so repeatedly.\n"
	with open(inPath, "w", encoding="utf-8", newline="\n") as vFile:
		vFile.write(vLine * (inSize // len(vLine) + 1))


def build():
	"""Write the whole fixture, replacing any previous run, and return a manifest."""
	if os.path.isdir(kOutDir):
		shutil.rmtree(kOutDir)
	os.makedirs(kOutDir)

	vManifest = []

	# The crash-shaped arm: zips at the chunk boundary, then two big ones.
	for vName, vSize in kStraddle + kBulk:
		vPath = os.path.join(kOutDir, f"zip-{vName}.zip")
		write_zip(vPath, vSize)
		vManifest.append(vPath)

	# The same sizes again with no zip container, so the two can be told apart.
	for vName, vSize in kStraddle:
		vPath = os.path.join(kOutDir, f"random-{vName}.bin")
		write_raw(vPath, vSize)
		vManifest.append(vPath)

	# Extensions a server or a client might treat differently, all incompressible underneath.
	for vExt in ["png", "jpg", "pdf", "tar.gz", "7z"]:
		vPath = os.path.join(kOutDir, f"opaque-{vExt.replace('.', '-')}.{vExt}")
		write_raw(vPath, kChunk + 512)
		vManifest.append(vPath)

	# The control arm: compressible text, so a failure that spares these points at compression.
	for vIndex in range(6):
		vPath = os.path.join(kOutDir, f"text-{vIndex:02d}.txt")
		write_text(vPath, kChunk)
		vManifest.append(vPath)

	# Pad to 30 files in the flat folder, so one drag is 30 uploads + 1 destination pre-scan = 31
	# channel opens. Padding is zipped random, keeping the fixture's weight on the suspect shape.
	vIndex = 0
	while len(vManifest) < 30:
		vPath = os.path.join(kOutDir, f"pad-{vIndex:02d}.zip")
		write_zip(vPath, kChunk)
		vManifest.append(vPath)
		vIndex += 1

	# A separate subtree for the folder-drop path, which walks recursively instead of batching.
	vTree = os.path.join(kOutDir, kTreeDir)
	for vSub in ["", "a", os.path.join("a", "b"), os.path.join("a", "b", "c")]:
		vDir = os.path.join(vTree, vSub) if vSub else vTree
		os.makedirs(vDir, exist_ok=True)
		for vIndex in range(3):
			write_zip(os.path.join(vDir, f"deep-{vIndex}.zip"), kChunk)

	return vManifest, vTree


def main():
	random.seed(0xC0FFEE)   # reproducible fixture: the same bytes every run
	vManifest, vTree = build()

	vFlatBytes = sum(os.path.getsize(vPath) for vPath in vManifest)
	vTreeFiles = sum(len(vFiles) for _, _, vFiles in os.walk(vTree))
	vTreeBytes = sum(
		os.path.getsize(os.path.join(vRoot, vName))
		for vRoot, _, vFiles in os.walk(vTree)
		for vName in vFiles
	)

	print(f"fixture: {kOutDir}")
	print(f"  flat files : {len(vManifest)}  ({vFlatBytes / (1 << 20):.1f} MiB)")
	print(f"  tree files : {vTreeFiles} under {kTreeDir}  ({vTreeBytes / (1 << 20):.1f} MiB)")
	print(f"  one drag of the flat files = {len(vManifest)} uploads + 1 pre-scan")
	vZips = sum(1 for vPath in vManifest if vPath.endswith(".zip"))
	print(f"  incompressible: {vZips} zips + 10 raw/opaque, compressible control: 6 text")


if __name__ == "__main__":
	main()
