#!/usr/bin/env python3
"""
Prepares the CelebA face set for the Familiar trainer.

What it does:
  1. Opens the official CelebA page (MMLab CUHK), takes the link to the Google Drive
     folder from it (together with the resourcekey access key), finds and downloads
     only the three files that are needed:
       img_align_celeba.zip     ~1.4 GB  aligned photos
       identity_CelebA.txt      ~3 MB    which person each photo belongs to
       list_attr_celeba.txt     ~25 MB   attributes: glasses, beard, hat, hair colour...
     The total download size is limited (3 GB by default). If a file is larger than
     what is left of the limit, the download is not started or is interrupted.
  2. Selects the people who have at least --min-photos photos.
  3. For each of them takes up to --per-person pictures, trying to make them differ
     in appearance as much as possible (glasses / none, beard / none, another hair
     colour and so on).
  4. Sorts the photos into folders: <out>/faces/<person number>/<file>.jpg
     and writes <out>/faces/manifest.json with the attributes of every photo.

Files already downloaded into the raw folder are not downloaded again. The archive
can be left as it is (img_align_celeba.zip) or unpacked (the img_align_celeba folder).

Only the Python 3.8+ standard library, nothing needs to be installed.

Examples:
  python prepare_celeba.py
  python prepare_celeba.py --people 600            # a compact selection for a phone
  python prepare_celeba.py --delete-zip            # delete the archive when done
  python prepare_celeba.py --skip-download         # the files are already in <out>/raw

CelebA licence: non-commercial research use only, copying and redistribution are
forbidden. The set is for personal training; do not pass it on to others.
"""

import argparse
import hashlib
import html
import http.cookiejar
import json
import os
import random
import re
import shutil
import sys
import time
import urllib.parse
import urllib.request
import zipfile

GB = 1_000_000_000
PAGE_URL = "https://mmlab.ie.cuhk.edu.hk/projects/CelebA.html"

# The Google Drive folder from the CelebA page (in case the page cannot be read).
FALLBACK_FOLDER = ("0B7EVK8r0v71pWEZsZE9oNnFzTm8", "0-5BR16BdXnb8hVj6CNHKzLg")

# Needed files: name -> (expected MD5, approximate size, fallback Drive ID)
NEEDED = {
    "identity_CelebA.txt": ("32bd1bd63d3c78cd57e08160ec5ed1e2", 3_500_000, "1_ee_0u7vcNLOfNLegJRHmolfH5ICW-XS"),
    "list_attr_celeba.txt": ("75e246fa4810816ffd6ee81facbd244c", 27_000_000, "0B7EVK8r0v71pblRyaVFSWGxPY0U"),
    "img_align_celeba.zip": ("00d2c5bc6d35e252742224ab0c1e8fcb", 1_450_000_000, "0B7EVK8r0v71pZjFTYXZWM3FlRnM"),
}
ORDER = ["identity_CelebA.txt", "list_attr_celeba.txt", "img_align_celeba.zip"]

# Attributes that change the appearance. Different pictures of one person are picked by them.
LOOK_ATTRS = [
    "Eyeglasses", "Wearing_Hat", "No_Beard", "Mustache", "Goatee", "Sideburns",
    "Bald", "Bangs", "Black_Hair", "Blond_Hair", "Brown_Hair", "Gray_Hair",
    "Heavy_Makeup", "Wearing_Lipstick", "Smiling", "Mouth_Slightly_Open",
    "Straight_Hair", "Wavy_Hair", "Receding_Hairline",
]
# Attributes that are kept in manifest.json for the difficulty levels of the app.
KEEP_ATTRS = LOOK_ATTRS + ["Male", "Young", "Pale_Skin", "Blurry"]

UA = ("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/124.0 Safari/537.36")
_opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))


class DownloadError(Exception):
    pass


class Budget:
    def __init__(self, limit_bytes):
        self.limit = limit_bytes
        self.used = 0

    @property
    def remaining(self):
        return self.limit - self.used


def human(n):
    for unit in ("B", "kB", "MB", "GB"):
        if n < 1000 or unit == "GB":
            return f"{n:.1f} {unit}" if unit != "B" else f"{n} {unit}"
        n /= 1000


def md5sum(path, chunk=4 * 1024 * 1024):
    h = hashlib.md5()
    with open(path, "rb") as f:
        while True:
            b = f.read(chunk)
            if not b:
                break
            h.update(b)
    return h.hexdigest()


def open_url(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    return _opener.open(req, timeout=60)


def get_text(url, limit=3_000_000):
    with open_url(url) as r:
        return r.read(limit).decode("utf-8", "replace")


# ---------- finding the files on Google Drive ----------

def find_root_folder():
    """Takes the link to the Drive folder (id + resourcekey) from the CelebA page."""
    try:
        page = html.unescape(get_text(PAGE_URL))
        m = re.search(r"drive\.google\.com/drive/folders/([\w-]+)\?resourcekey=([\w-]+)", page)
        if m:
            return m.group(1), m.group(2)
        m = re.search(r"drive\.google\.com/drive/folders/([\w-]+)", page)
        if m:
            return m.group(1), None
        print("  ! No Google Drive link found on the CelebA page, using the known folder")
    except Exception as e:
        print(f"  ! Could not open the CelebA page ({e}), using the known folder")
    return FALLBACK_FOLDER


def list_folder(folder_id, key):
    """Lists the items of a Drive folder: [(name, id, resourcekey, is_folder)]."""
    url = f"https://drive.google.com/embeddedfolderview?id={folder_id}"
    if key:
        url += f"&resourcekey={key}"
    page = get_text(url)
    items = []
    for chunk in page.split('class="flip-entry"')[1:]:
        mid = re.search(r'id="entry-([\w-]+)"', chunk)
        mhref = re.search(r'href="([^"]+)"', chunk)
        mtitle = re.search(r'class="flip-entry-title"[^>]*>([^<]+)<', chunk)
        if not (mid and mhref and mtitle):
            continue
        href = html.unescape(mhref.group(1))
        mkey = re.search(r"resourcekey=([\w-]+)", href)
        items.append((html.unescape(mtitle.group(1)).strip(), mid.group(1),
                      mkey.group(1) if mkey else None, "/folders/" in href))
    return items


def locate_files(names):
    """Walks the CelebA folder on Drive and returns {name: (id, resourcekey)} for the needed files."""
    root = find_root_folder()
    print(f"  Google Drive folder: {root[0]}" + (" (with an access key)" if root[1] else ""))
    found = {}
    queue = [(root[0], root[1], 0)]
    seen = set()
    while queue and len(found) < len(names):
        fid, key, depth = queue.pop(0)
        if fid in seen or depth > 3:
            continue
        seen.add(fid)
        try:
            items = list_folder(fid, key)
        except Exception as e:
            print(f"  ! Could not read folder {fid}: {e}")
            continue
        for name, iid, ikey, is_folder in items:
            if is_folder:
                # Folders with annotations and aligned photos go first
                pri = 0 if re.search(r"anno|img", name, re.I) else 1
                queue.insert(0, (iid, ikey or key, depth + 1)) if pri == 0 else queue.append((iid, ikey or key, depth + 1))
            elif name in names and name not in found:
                found[name] = (iid, ikey)
    return found


# ---------- downloading ----------

# Matched against the page Google returns; the Russian phrases cover its Russian locale.
QUOTA_MARKERS = ("quota exceeded", "too many users have viewed or downloaded",
                 "квота превышена", "превышена квота", "слишком много пользователей")
DEBUG_HTML = None  # where the last response from Google that was a page is saved


def _stream_to_file(r, tmp, budget):
    size = r.headers.get("Content-Length")
    if size is not None and int(size) > budget.remaining:
        raise DownloadError(
            f"The file is {human(int(size))}, but only {human(budget.remaining)} of the limit is left. "
            "The download was not started."
        )
    total = int(size) if size else None
    done, t0, last = 0, time.time(), 0.0
    with open(tmp, "wb") as f:
        while True:
            chunk = r.read(1024 * 1024)
            if not chunk:
                break
            done += len(chunk)
            if done > budget.remaining:
                raise DownloadError(f"The download limit ({human(budget.limit)}) was exceeded. The download was interrupted.")
            f.write(chunk)
            now = time.time()
            if now - last > 0.5:
                last = now
                speed = done / max(now - t0, 1e-6)
                pct = f"{done / total * 100:5.1f}%" if total else "     "
                sys.stdout.write(f"\r    {pct}  {human(done)}  {human(speed)}/s   ")
                sys.stdout.flush()
    sys.stdout.write("\n")
    return done


def _confirm_url(page):
    """Builds the confirmation link from the "could not scan for viruses" page."""
    m = re.search(r'<form[^>]*id="download-form"[^>]*action="([^"]+)"', page)
    if not m:
        return None
    action = html.unescape(m.group(1))
    params = dict(re.findall(r'<input[^>]*type="hidden"[^>]*name="([^"]+)"[^>]*value="([^"]*)"', page))
    return action + "?" + urllib.parse.urlencode(params)


def drive_download(file_id, key, dest, budget):
    url = f"https://drive.usercontent.google.com/download?id={file_id}&export=download&confirm=t"
    if key:
        url += f"&resourcekey={key}"
    tmp = dest + ".part"
    try:
        for attempt in range(2):
            with open_url(url) as r:
                if "text/html" not in r.headers.get("Content-Type", ""):
                    done = _stream_to_file(r, tmp, budget)
                    break
                page = r.read(500_000).decode("utf-8", "replace")
            if DEBUG_HTML:
                try:
                    with open(DEBUG_HTML, "w", encoding="utf-8") as fh:
                        fh.write(page)
                except OSError:
                    pass
            low = page.lower()
            if any(mk in low for mk in QUOTA_MARKERS):
                raise DownloadError("Google Drive says the download quota of this file is used up. Try again in a few hours.")
            nxt = _confirm_url(page)
            if not nxt or attempt == 1:
                title = re.search(r"<title>([^<]*)</title>", page)
                raise DownloadError("Google Drive returned a page instead of the file"
                                    + (f" (\"{title.group(1).strip()}\")" if title else "") + ".")
            url = nxt
    except DownloadError:
        if os.path.exists(tmp):
            os.remove(tmp)
        raise
    except Exception as e:
        if os.path.exists(tmp):
            os.remove(tmp)
        raise DownloadError(f"Network error: {e}") from e

    with open(tmp, "rb") as f:
        head = f.read(512).lstrip().lower()
    if head.startswith(b"<!doctype html") or head.startswith(b"<html"):
        os.remove(tmp)
        raise DownloadError("Google Drive returned a page instead of the file.")
    os.replace(tmp, dest)
    budget.used += done
    return done


def have_file(raw_dir, name):
    path = os.path.join(raw_dir, name)
    if os.path.exists(path) and os.path.getsize(path) > 0:
        return True
    if name == "img_align_celeba.zip" and os.path.isdir(os.path.join(raw_dir, "img_align_celeba")):
        return True
    return False


def ensure_files(raw_dir, budget, skip_download):
    os.makedirs(raw_dir, exist_ok=True)
    missing = [n for n in ORDER if not have_file(raw_dir, n)]
    for n in ORDER:
        if n not in missing:
            print(f"  ✓ {n} is already there, skipping the download")
    if not missing:
        return
    if skip_download:
        raise DownloadError(f"Missing files: {', '.join(missing)}. Put them into {raw_dir}")

    need = sum(NEEDED[n][1] for n in missing)
    if need > budget.remaining:
        raise DownloadError(f"The needed files (~{human(need)}) do not fit into the limit of {human(budget.remaining)}.")

    global DEBUG_HTML
    DEBUG_HTML = os.path.join(raw_dir, "drive_response.html")
    print("  Looking for the files in the CelebA folder on Google Drive…")
    located = locate_files(set(missing))
    for n in missing:
        if n not in located:
            print(f"  ! {n} was not found in the folder, trying the known direct link")
            located[n] = (NEEDED[n][2], None)

    for n in missing:
        md5, approx, _ = NEEDED[n]
        fid, key = located[n]
        path = os.path.join(raw_dir, n)
        print(f"  ↓ {n} (~{human(approx)}), {human(budget.remaining)} of the limit left")
        try:
            drive_download(fid, key, path, budget)
        except DownloadError as e:
            raise DownloadError(
                f"{n}: {e}\n"
                f"  The response from Google was saved to {DEBUG_HTML}\n"
                f"  What to do: open {PAGE_URL}, section Downloads, download {n} by hand "
                f"and put it into {raw_dir}, then run the script again "
                f"(what is already downloaded will not be downloaded again)."
            ) from e
        got = md5sum(path)
        print("    MD5 matches" if got == md5 else
              f"    ! MD5 does not match ({got}). The authors may have updated the file; continuing.")


# ---------- processing ----------

def read_identities(path):
    by_person = {}
    with open(path, "r", encoding="utf-8") as f:
        for line in f:
            parts = line.split()
            if len(parts) == 2:
                by_person.setdefault(parts[1], []).append(parts[0])
    return by_person


def read_attrs(path):
    with open(path, "r", encoding="utf-8") as f:
        f.readline()  # the number of lines
        names = f.readline().split()
        idx = {n: i for i, n in enumerate(names)}
        keep = [(n, idx[n]) for n in KEEP_ATTRS if n in idx]
        attrs = {}
        for line in f:
            parts = line.split()
            if len(parts) != len(names) + 1:
                continue
            vals = parts[1:]
            attrs[parts[0]] = {n: 1 if vals[i] == "1" else 0 for n, i in keep}
    return attrs


class ImageSource:
    """Reads photos from the archive or from the unpacked folder."""

    def __init__(self, raw_dir):
        zpath = os.path.join(raw_dir, "img_align_celeba.zip")
        dpath = os.path.join(raw_dir, "img_align_celeba")
        self.zip = None
        if os.path.exists(zpath):
            self.zip = zipfile.ZipFile(zpath)
            self.index = {os.path.basename(n): n for n in self.zip.namelist() if n.lower().endswith(".jpg")}
        else:
            self.index = {}
            for root, _, files in os.walk(dpath):
                for f in files:
                    if f.lower().endswith(".jpg"):
                        self.index[f] = os.path.join(root, f)

    def read(self, name):
        if self.zip:
            return self.zip.read(self.index[name])
        with open(self.index[name], "rb") as f:
            return f.read()

    def close(self):
        if self.zip:
            self.zip.close()


def pick_diverse(files, attrs, k, rnd):
    """Greedily picks k photos that differ in appearance attributes as much as possible."""
    if len(files) <= k:
        return list(files)
    vec = {f: tuple(attrs.get(f, {}).get(a, 0) for a in LOOK_ATTRS) for f in files}
    pool = list(files)
    rnd.shuffle(pool)
    chosen = [pool.pop()]
    while len(chosen) < k and pool:
        best = max(pool, key=lambda f: min(sum(x != y for x, y in zip(vec[f], vec[c])) for c in chosen))
        chosen.append(best)
        pool.remove(best)
    return chosen


def main():
    ap = argparse.ArgumentParser(description="Prepares CelebA for the Familiar trainer")
    ap.add_argument("--out", default="celeba_trainer", help="output folder (default ./celeba_trainer)")
    ap.add_argument("--raw", default=None, help="folder for the downloaded files (default <out>/raw)")
    ap.add_argument("--people", type=int, default=1500, help="how many people to select (1500; 600 for a phone)")
    ap.add_argument("--per-person", type=int, default=15, help="maximum photos per person (15)")
    ap.add_argument("--min-photos", type=int, default=10, help="minimum photos a person must have in the source set (10)")
    ap.add_argument("--max-download-gb", type=float, default=3.0, help="download limit in GB (3)")
    ap.add_argument("--seed", type=int, default=42, help="seed of the random selection")
    ap.add_argument("--skip-download", action="store_true", help="download nothing, the files are already in --raw")
    ap.add_argument("--delete-zip", action="store_true", help="delete the photo archive when done")
    args = ap.parse_args()

    out = os.path.abspath(os.path.expanduser(args.out))
    raw = os.path.abspath(os.path.expanduser(args.raw or os.path.join(out, "raw")))
    faces = os.path.join(out, "faces")
    budget = Budget(int(args.max_download_gb * GB))

    os.makedirs(out, exist_ok=True)
    print(f"Output: {out}")
    print(f"Download limit: {human(budget.limit)}")

    need = sum(NEEDED[n][1] for n in ORDER if not have_file(raw, n)) + 500_000_000
    free = shutil.disk_usage(out).free
    if free < need:
        print(f"Not enough disk space: {human(free)} free, about {human(need)} needed.")
        sys.exit(1)

    print("\n1. CelebA files")
    try:
        ensure_files(raw, budget, args.skip_download)
    except DownloadError as e:
        print(f"\nStopped: {e}")
        sys.exit(2)
    print(f"  Downloaded in this run: {human(budget.used)}")

    print("\n2. Annotations")
    by_person = read_identities(os.path.join(raw, "identity_CelebA.txt"))
    attrs = read_attrs(os.path.join(raw, "list_attr_celeba.txt"))
    eligible = sorted((p for p, fs in by_person.items() if len(fs) >= args.min_photos), key=int)
    print(f"  People in total: {len(by_person)}, with {args.min_photos}+ photos: {len(eligible)}")
    rnd = random.Random(args.seed)
    rnd.shuffle(eligible)
    selected = eligible[: args.people]
    print(f"  Selected: {len(selected)}")

    print("\n3. Selecting and sorting the photos")
    if os.path.exists(faces):
        shutil.rmtree(faces)
    os.makedirs(faces)
    src = ImageSource(raw)
    if not src.index:
        print("  No photos found in img_align_celeba.zip or in the img_align_celeba folder.")
        sys.exit(3)
    manifest = {"source": "CelebA (MMLab CUHK), non-commercial research use only",
                "attrs": KEEP_ATTRS, "people": []}
    written = total_bytes = 0
    for i, pid in enumerate(selected, 1):
        files = [f for f in by_person[pid] if f in src.index]
        chosen = pick_diverse(files, attrs, args.per_person, rnd)
        pdir = os.path.join(faces, pid)
        os.makedirs(pdir, exist_ok=True)
        photos = []
        for f in chosen:
            data = src.read(f)
            with open(os.path.join(pdir, f), "wb") as fh:
                fh.write(data)
            total_bytes += len(data)
            written += 1
            photos.append({"file": f"{pid}/{f}", "attrs": attrs.get(f, {})})
        manifest["people"].append({"id": pid, "photos": photos})
        if i % 50 == 0 or i == len(selected):
            sys.stdout.write(f"\r  {i}/{len(selected)} people, {written} photos, {human(total_bytes)}   ")
            sys.stdout.flush()
    sys.stdout.write("\n")
    src.close()
    with open(os.path.join(faces, "manifest.json"), "w", encoding="utf-8") as fh:
        json.dump(manifest, fh, ensure_ascii=False, separators=(",", ":"))

    if args.delete_zip:
        zpath = os.path.join(raw, "img_align_celeba.zip")
        if os.path.exists(zpath):
            os.remove(zpath)
            print(f"  Archive deleted: {zpath}")

    print("\nDone.")
    print(f"  Folder for the trainer: {faces}")
    print(f"  {len(selected)} people, {written} photos, {human(total_bytes)}")


if __name__ == "__main__":
    main()
