#!/usr/bin/env python3
"""
Подготовка набора CelebA для тренажёра «Опознание».

Что делает:
  1. Открывает официальную страницу CelebA (MMLab CUHK), берёт с неё ссылку на папку
     Google Drive (вместе с ключом доступа resourcekey), находит в папке и скачивает
     только три нужных файла:
       img_align_celeba.zip     ~1.4 ГБ  выровненные фото
       identity_CelebA.txt      ~3 МБ    какому человеку принадлежит фото
       list_attr_celeba.txt     ~25 МБ   признаки: очки, борода, шляпа, цвет волос...
     Суммарный объём скачивания ограничен (по умолчанию 3 ГБ). Если файл больше
     оставшегося лимита, скачивание не начинается или прерывается.
  2. Отбирает людей, у которых не меньше --min-photos фото.
  3. Для каждого берёт до --per-person снимков, стараясь, чтобы они максимально
     различались по облику (очки / без, борода / без, другой цвет волос и т. д.).
  4. Раскладывает фото по папкам: <out>/faces/<номер человека>/<файл>.jpg
     и пишет <out>/faces/manifest.json с признаками каждого фото.

Уже скачанные файлы в папке raw не скачиваются повторно. Архив можно положить
как есть (img_align_celeba.zip) или распакованным (папка img_align_celeba).

Только стандартная библиотека Python 3.8+, ничего устанавливать не нужно.

Примеры:
  python prepare_celeba.py
  python prepare_celeba.py --people 600            # компактная выборка для телефона
  python prepare_celeba.py --delete-zip            # удалить архив после обработки
  python prepare_celeba.py --skip-download         # файлы уже лежат в <out>/raw

Лицензия CelebA: только некоммерческое исследовательское использование,
копирование и распространение запрещены. Набор — для личной тренировки, не передавай его другим.
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

# Папка Google Drive со страницы CelebA (на случай, если страницу не удастся прочитать).
FALLBACK_FOLDER = ("0B7EVK8r0v71pWEZsZE9oNnFzTm8", "0-5BR16BdXnb8hVj6CNHKzLg")

# Нужные файлы: имя -> (ожидаемый MD5, примерный размер, запасной Drive ID)
NEEDED = {
    "identity_CelebA.txt": ("32bd1bd63d3c78cd57e08160ec5ed1e2", 3_500_000, "1_ee_0u7vcNLOfNLegJRHmolfH5ICW-XS"),
    "list_attr_celeba.txt": ("75e246fa4810816ffd6ee81facbd244c", 27_000_000, "0B7EVK8r0v71pblRyaVFSWGxPY0U"),
    "img_align_celeba.zip": ("00d2c5bc6d35e252742224ab0c1e8fcb", 1_450_000_000, "0B7EVK8r0v71pZjFTYXZWM3FlRnM"),
}
ORDER = ["identity_CelebA.txt", "list_attr_celeba.txt", "img_align_celeba.zip"]

# Признаки, по которым меняется облик. По ним подбираются разные снимки одного человека.
LOOK_ATTRS = [
    "Eyeglasses", "Wearing_Hat", "No_Beard", "Mustache", "Goatee", "Sideburns",
    "Bald", "Bangs", "Black_Hair", "Blond_Hair", "Brown_Hair", "Gray_Hair",
    "Heavy_Makeup", "Wearing_Lipstick", "Smiling", "Mouth_Slightly_Open",
    "Straight_Hair", "Wavy_Hair", "Receding_Hairline",
]
# Признаки, которые сохраняются в manifest.json для уровней сложности в приложении.
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
    for unit in ("Б", "КБ", "МБ", "ГБ"):
        if n < 1000 or unit == "ГБ":
            return f"{n:.1f} {unit}" if unit != "Б" else f"{n} {unit}"
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


# ---------- поиск файлов на Google Drive ----------

def find_root_folder():
    """Берёт ссылку на папку Drive (id + resourcekey) со страницы CelebA."""
    try:
        page = html.unescape(get_text(PAGE_URL))
        m = re.search(r"drive\.google\.com/drive/folders/([\w-]+)\?resourcekey=([\w-]+)", page)
        if m:
            return m.group(1), m.group(2)
        m = re.search(r"drive\.google\.com/drive/folders/([\w-]+)", page)
        if m:
            return m.group(1), None
        print("  ! На странице CelebA не нашлась ссылка на Google Drive, беру известную папку")
    except Exception as e:
        print(f"  ! Не удалось открыть страницу CelebA ({e}), беру известную папку")
    return FALLBACK_FOLDER


def list_folder(folder_id, key):
    """Список элементов папки Drive: [(имя, id, resourcekey, это_папка)]."""
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
    """Обходит папку CelebA на Drive и возвращает {имя: (id, resourcekey)} для нужных файлов."""
    root = find_root_folder()
    print(f"  Папка Google Drive: {root[0]}" + (" (с ключом доступа)" if root[1] else ""))
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
            print(f"  ! Не удалось прочитать папку {fid}: {e}")
            continue
        for name, iid, ikey, is_folder in items:
            if is_folder:
                # Сначала папки с аннотациями и выровненными фото
                pri = 0 if re.search(r"anno|img", name, re.I) else 1
                queue.insert(0, (iid, ikey or key, depth + 1)) if pri == 0 else queue.append((iid, ikey or key, depth + 1))
            elif name in names and name not in found:
                found[name] = (iid, ikey)
    return found


# ---------- скачивание ----------

QUOTA_MARKERS = ("quota exceeded", "too many users have viewed or downloaded",
                 "квота превышена", "превышена квота", "слишком много пользователей")
DEBUG_HTML = None  # путь, куда сохраняется последний ответ Google в виде страницы


def _stream_to_file(r, tmp, budget):
    size = r.headers.get("Content-Length")
    if size is not None and int(size) > budget.remaining:
        raise DownloadError(
            f"Файл весит {human(int(size))}, а до лимита осталось {human(budget.remaining)}. "
            "Скачивание не начато."
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
                raise DownloadError(f"Превышен лимит скачивания ({human(budget.limit)}). Скачивание прервано.")
            f.write(chunk)
            now = time.time()
            if now - last > 0.5:
                last = now
                speed = done / max(now - t0, 1e-6)
                pct = f"{done / total * 100:5.1f}%" if total else "     "
                sys.stdout.write(f"\r    {pct}  {human(done)}  {human(speed)}/с   ")
                sys.stdout.flush()
    sys.stdout.write("\n")
    return done


def _confirm_url(page):
    """Из страницы «не удалось проверить на вирусы» собирает ссылку подтверждения."""
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
                raise DownloadError("Google Drive сообщает, что квота скачиваний этого файла исчерпана. Попробуй через несколько часов.")
            nxt = _confirm_url(page)
            if not nxt or attempt == 1:
                title = re.search(r"<title>([^<]*)</title>", page)
                raise DownloadError("Google Drive вернул страницу вместо файла"
                                    + (f" («{title.group(1).strip()}»)" if title else "") + ".")
            url = nxt
    except DownloadError:
        if os.path.exists(tmp):
            os.remove(tmp)
        raise
    except Exception as e:
        if os.path.exists(tmp):
            os.remove(tmp)
        raise DownloadError(f"Ошибка сети: {e}") from e

    with open(tmp, "rb") as f:
        head = f.read(512).lstrip().lower()
    if head.startswith(b"<!doctype html") or head.startswith(b"<html"):
        os.remove(tmp)
        raise DownloadError("Google Drive вернул страницу вместо файла.")
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
            print(f"  ✓ {n} уже есть, пропускаю скачивание")
    if not missing:
        return
    if skip_download:
        raise DownloadError(f"Нет файлов: {', '.join(missing)}. Положи их в {raw_dir}")

    need = sum(NEEDED[n][1] for n in missing)
    if need > budget.remaining:
        raise DownloadError(f"Нужные файлы (~{human(need)}) не помещаются в лимит {human(budget.remaining)}.")

    global DEBUG_HTML
    DEBUG_HTML = os.path.join(raw_dir, "drive_response.html")
    print("  Ищу файлы в папке CelebA на Google Drive…")
    located = locate_files(set(missing))
    for n in missing:
        if n not in located:
            print(f"  ! {n} не найден в папке, попробую известную прямую ссылку")
            located[n] = (NEEDED[n][2], None)

    for n in missing:
        md5, approx, _ = NEEDED[n]
        fid, key = located[n]
        path = os.path.join(raw_dir, n)
        print(f"  ↓ {n} (~{human(approx)}), лимит осталось {human(budget.remaining)}")
        try:
            drive_download(fid, key, path, budget)
        except DownloadError as e:
            raise DownloadError(
                f"{n}: {e}\n"
                f"  Ответ Google сохранён в {DEBUG_HTML}\n"
                f"  Что сделать: открой {PAGE_URL}, раздел Downloads, скачай {n} вручную "
                f"и положи в папку {raw_dir}, затем запусти скрипт ещё раз "
                f"(уже скачанное повторно качаться не будет)."
            ) from e
        got = md5sum(path)
        print("    MD5 совпал" if got == md5 else
              f"    ! MD5 не совпал ({got}). Возможно, авторы обновили файл; продолжаю.")


# ---------- обработка ----------

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
        f.readline()  # количество строк
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
    """Читает фото из архива или из распакованной папки."""

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
    """Жадно отбирает k фото, максимально различающихся по признакам облика."""
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
    ap = argparse.ArgumentParser(description="Подготовка CelebA для тренажёра «Опознание»")
    ap.add_argument("--out", default="celeba_trainer", help="папка результата (по умолчанию ./celeba_trainer)")
    ap.add_argument("--raw", default=None, help="папка для скачанных файлов (по умолчанию <out>/raw)")
    ap.add_argument("--people", type=int, default=1500, help="сколько человек отобрать (1500; для телефона 600)")
    ap.add_argument("--per-person", type=int, default=15, help="максимум фото на человека (15)")
    ap.add_argument("--min-photos", type=int, default=10, help="минимум фото у человека в исходном наборе (10)")
    ap.add_argument("--max-download-gb", type=float, default=3.0, help="лимит скачивания в ГБ (3)")
    ap.add_argument("--seed", type=int, default=42, help="зерно случайного отбора")
    ap.add_argument("--skip-download", action="store_true", help="ничего не скачивать, файлы уже лежат в --raw")
    ap.add_argument("--delete-zip", action="store_true", help="удалить архив с фото после обработки")
    args = ap.parse_args()

    out = os.path.abspath(os.path.expanduser(args.out))
    raw = os.path.abspath(os.path.expanduser(args.raw or os.path.join(out, "raw")))
    faces = os.path.join(out, "faces")
    budget = Budget(int(args.max_download_gb * GB))

    os.makedirs(out, exist_ok=True)
    print(f"Результат: {out}")
    print(f"Лимит скачивания: {human(budget.limit)}")

    need = sum(NEEDED[n][1] for n in ORDER if not have_file(raw, n)) + 500_000_000
    free = shutil.disk_usage(out).free
    if free < need:
        print(f"Мало места на диске: свободно {human(free)}, нужно примерно {human(need)}.")
        sys.exit(1)

    print("\n1. Файлы CelebA")
    try:
        ensure_files(raw, budget, args.skip_download)
    except DownloadError as e:
        print(f"\nОстановлено: {e}")
        sys.exit(2)
    print(f"  Скачано за этот запуск: {human(budget.used)}")

    print("\n2. Разметка")
    by_person = read_identities(os.path.join(raw, "identity_CelebA.txt"))
    attrs = read_attrs(os.path.join(raw, "list_attr_celeba.txt"))
    eligible = sorted((p for p, fs in by_person.items() if len(fs) >= args.min_photos), key=int)
    print(f"  Всего людей: {len(by_person)}, с {args.min_photos}+ фото: {len(eligible)}")
    rnd = random.Random(args.seed)
    rnd.shuffle(eligible)
    selected = eligible[: args.people]
    print(f"  Отобрано: {len(selected)}")

    print("\n3. Отбор и раскладка фото")
    if os.path.exists(faces):
        shutil.rmtree(faces)
    os.makedirs(faces)
    src = ImageSource(raw)
    if not src.index:
        print("  Не нашёл фото ни в img_align_celeba.zip, ни в папке img_align_celeba.")
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
            sys.stdout.write(f"\r  {i}/{len(selected)} человек, {written} фото, {human(total_bytes)}   ")
            sys.stdout.flush()
    sys.stdout.write("\n")
    src.close()
    with open(os.path.join(faces, "manifest.json"), "w", encoding="utf-8") as fh:
        json.dump(manifest, fh, ensure_ascii=False, separators=(",", ":"))

    if args.delete_zip:
        zpath = os.path.join(raw, "img_align_celeba.zip")
        if os.path.exists(zpath):
            os.remove(zpath)
            print(f"  Архив удалён: {zpath}")

    print("\nГотово.")
    print(f"  Папка для тренажёра: {faces}")
    print(f"  {len(selected)} человек, {written} фото, {human(total_bytes)}")


if __name__ == "__main__":
    main()
