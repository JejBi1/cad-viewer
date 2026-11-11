# Štíhlé OCP (cadquery-ocp-slim) – Open CASCADE 8.0.1 jen pro CAD prohlížeč

Od verze 1.9 program nepoužívá `cadquery-ocp` (321 modulů + VTK, 300 MB DLL / 780 MB `.so`), ale vlastní sestavení
téhož generátoru (CadQuery/OCP 8.0.1.1) omezené na **73 modulů**, které program opravdu volá, proti conda-forge `occt=8.0.1`
(varianta `novtk` – bez VTK a TBB). Importní jména jsou `OCP` a `OCP.libs`, takže se program nemění; balíček
nemá závislosti a **kolizí** s `cadquery-ocp` (oba dávají balíček `OCP`) – ve stejném venv nesmí být oba ani `vtk`.

| | cadquery-ocp 8.0.1.1.0 | štíhlé OCP |
|---|---|---|
| import `OCP` + moduly programu | Linux 1,33 s, Windows 1,26 s | 0,40 s / 0,40 s |
| Windows: OCP.pyd + DLL | 302 MB (102 DLL) | 86 MB (56 DLL), wheel 29,6 MB |
| Linux: OCP.so + knihovny | 778 MB (134) | 122 MB (57), wheel 40,1 MB |
| složka programu (PyInstaller) | Windows 438 MB, Linux 803 MB | 215 MB, 289 MB |

Výsledky se zkoušely proti originálu: geometrie (počty ploch, objemy, těžiště, vzdálenosti, kolize), náhledy PNG
(pixel po pixelu shodné) i okno programu (`scripts/gui_drive.py`, 13 kroků, 4 snímky shodné bajt po bajtu).
Chybí 1,76 % členů sdílených tříd (`scripts/api_diff.py`), vše kvůli ořezání; program nic z toho nepoužívá
(`IGESModel`, `AttributeDrivers`, `ChangeReader` …).

## Kde co leží
- `wheels/` – hotové wheely (Windows `win_amd64`, Linux `linux_x86_64`, oba Python 3.11). **Instalace do sestavovacího
  prostředí:** `pip uninstall -y cadquery-ocp cadquery-ocp-proxy vtk`, pak `pip install --no-deps wheels/<wheel>`.
  Používají je `build.py` (venv `%USERPROFILE%\.venvs\stepviewer`) a `linux/setup_build_env.sh` (`/opt/venv`).
- `generated/` – vygenerované C++ (nejdražší část, hodiny): `OCP_slim_cpp_{linux,windows}.tar.gz` + `slim_linux.toml`
  (seznam modulů pro generátor). S nimi stačí jen kompilace a balení.
- `scripts/` – všechny skripty sestavení a ověření (viz níže), `data/` – `modules_{linux,windows}.txt` (73 modulů),
  `app_imports.json` (co program importuje z OCP), `used_names.txt` (názvy metod, které program volá).
- `venv_{windows,linux}_before_slim_freeze.txt` – `pip freeze` sestavovacích prostředí před přechodem. Návrat k originálu:
  `pip uninstall -y cadquery-ocp-slim` a `pip install cadquery-ocp==8.0.1.1.0 cadquery-ocp-proxy==8.0.1.1.0 vtk==9.6.2`
  (a ve starém `build.py` / `build_linux.sh` z verze 1.8 je `cadquery_ocp.libs` + `vtk.libs`).

## Windows: co se děje v sestavení
- `build.py`: PyInstaller najde `OCP.libs` vedle balíčku sám (žádné `--add-binary`, jinak by DLL byly ve složce dvakrát).
- DLL z conda jsou přeložené novějším MSVC (14.51) než běh, který PyInstaller bere z Pythonu (14.38) a PySide6 (14.44).
  Proces načte jeden `msvcp140.dll` / `vcruntime140.dll` bez ohledu na složku a C++ běh je kompatibilní jen směrem nahoru,
  proto `build.py` po sestavení nahradí všechny starší kopie těchto DLL nejnovější (`unify_msvc_runtime`).

## Linux: co se děje v sestavení
- `OCP.libs/` obsahuje vlastní `libstdc++.so.6` a `libgcc_s.so.1` z gcc 15 (conda OCCT potřebuje `CXXABI_1.3.15`). Dva
  různé `libstdc++` v jednom procesu spadnou, proto `linux/build_linux.sh` po PyInstalleru přepíše kopii v `_internal/`
  těmito dvěma. Ve venv (bez PyInstalleru) je potřeba `LD_PRELOAD=$SP/OCP.libs/libstdc++.so.6:$SP/OCP.libs/libgcc_s.so.1`.
- Cizí knihovny (jpeg, png, tiff, zlib …) mají soukromé SONAME (`ocpjpeg.so.8` …), aby je PyInstaller nesplácl s jinými
  se stejným jménem (jinak `libtiff: undefined symbol jpeg12_write_raw_data`).
- `OCP.so` vyžaduje glibc ≥ 2.34 (program kvůli Qt stejně glibc 2.35).

## Znovu sestavit (přesný postup)
Potřeba: **Windows** – Visual Studio 2022 Build Tools (MSVC 14.44, C++ + Windows SDK), micromamba
(`%LOCALAPPDATA%\micromamba\micromamba.exe`), WSL s distem `cadbuild` (Ubuntu 22.04, ≥ 16 vláken, 11 GB RAM; ninja max. 9 úloh –
dvě jednotky `AIS.cpp` a jedna `XCAFDoc` potřebují po ~4 GB); **Linux** – micromamba v `/opt/micromamba/bin/micromamba`
(v WSL), conda gcc 15 (z prostředí `b311`). Cesty ve skriptech počítají s pracovní složkou `/root/slimocp` (WSL) resp.
`C:\slimocp_w` (Windows; **krátká cesta je nutnost**, `LongPathsEnabled=0`, cesty conda/MSVC přesahují 260 znaků).
Dlouhé kroky ve WSL spouštět jako `wsl -d cadbuild -u root -- bash -l -c "..."` a nechat okno otevřené (úlohy na pozadí
umírají se zavřením relace `wsl.exe`).

```
# 0. zdroje (WSL) – oficiální generátor a jeho submodul
git clone --depth 1 --branch 8.0.1.1 https://github.com/CadQuery/OCP.git /root/slimocp/OCP_src
git -C /root/slimocp/OCP_src submodule update --init --depth 1 pywrap
mkdir -p /root/slimocp && cp -r native/slimocp/scripts native/slimocp/data /root/slimocp/   # skripty a seznamy modulů

# 1. Linux, vše najednou:  scripts/build_all_linux.sh
#    (00_setup_envs_linux.sh, 01_parse_round.sh r1/r2/r3, closure2.py, 02_generate.sh, 03_build_linux.sh,
#     package_linux.py, make_wheel.py)                         -> /root/slimocp/out/wheels/*linux_x86_64.whl

# 2. Windows
powershell -File scripts\00_setup_env_windows.ps1                        # conda env bwin v C:\slimocp_w\mm
wsl ... bash /root/slimocp/scripts/01b_setup_windows_target.sh            # include MSVC + symbols_mangled_win.dat
wsl ... bash /root/slimocp/scripts/01_parse_round.sh w1 /root/slimocp/work/modules_windows.txt Windows 6      # 30–70 min
wsl ... bash /root/slimocp/scripts/02_generate.sh Windows /root/slimocp/work/modules_windows.txt /root/slimocp/work/windows/parse_w1.pkl
cp -r /root/slimocp/work/windows/gen/OCP /mnt/c/slimocp_w/gen/OCP
scripts\03_build_windows.bat 8                                            # vcvars64 + cmake + ninja, ~26 min
python scripts\package_windows.py --pyd C:\slimocp_w\build\OCP.cp311-win_amd64.pyd --modules modules_windows.txt ^
       --bindir C:\slimocp_w\mm\envs\bwin\Library\bin --out C:\slimocp_w\out\windows
python scripts\make_wheel.py C:\slimocp_w\out\windows C:\slimocp_w\out\wheels win_amd64
```
Hotové vygenerované C++ je v `generated/` – s ním se přeskočí kroky parse + generate a stačí rozbalit do
`/root/slimocp/work/<platforma>/gen/OCP` (resp. `C:\slimocp_w\gen\OCP`) a spustit `03_build_*` + balení.

**Přidání modulu** (kdyby program začal volat další část OCCT): připsat ho do `data/modules_*.txt`,
`01_parse_round.sh <jméno> <seznam> <platforma>` jen pro nový modul, znovu `02_generate.sh` se všemi pickly,
`03_build_*` (přeloží se jen změněné jednotky), balení. Kontrola, že program nic nepostrádá: `scripts/verify_app.py`,
`scripts/gui_drive.py` a `scripts/api_diff.py` (srovnání s originálním OCP).

## Jak je omezení modulů uděláno (pro orientaci)
1. **Parse** oficiálním `python -m bindgen parse` (libclang 21, hlavičky OCCT 8.0.1 z repozitáře OCP).
2. **Výběr modulů** (`closure2.py`): od všeho, co program importuje (`data/app_imports.json`, rozbor AST `stepview/*.py`),
   tranzitivně přes základní třídy a třídy v podpisech metod, jejichž NÁZEV program volá (`data/used_names.txt`);
   73 modulů včetně rezervy (`TopTools TColStd TColgp Precision math BVH`).
3. **Generování** (`gen_driver.py`): jako oficiální, navíc **ořezání** – konstruktory, metody, funkce a pole, které zmiňují
   třídu z modulu mimo sestavení, se vynechají (421 metod Linux / 415 Windows; hlavně IFSelect, XSControl, IGESControl,
   SelectMgr, STEPCAFControl, interní části BRepMesh). `PYTHONHASHSEED=0` dělá výstup deterministický.
4. **Překlad**: vlastní `CMakeLists.slim.txt` (bez VTK, `-O2 -g0`, MSVC `/bigobj /O2`), Linux conda gcc 15,
   Windows MSVC 14.44 proti conda `novtk` DLL. Windows se generuje z WSL (cross-target `x86_64-pc-windows-msvc`) jako
   oficiální CI, nativně se jen překládá.
5. **Balení** (`package_linux.py`, `package_windows.py`, `make_wheel.py`): zkopíruje se přesně uzávěr závislostí modulu
   (readelf / pefile) – z 51 knihoven TK jen 30 – a vytvoří se stejné zástupné balíčky `OCP/<Modul>/` jako v oficiálním wheelu.
6. Pozor: `pywrap` potřebuje Python ≥ 3.12 (generování, env `gen`), překlad běží v Pythonu 3.11 (env `b311`).
