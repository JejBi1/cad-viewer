# CAD prohlížeč 2.0

**Česky** | [English](README.en.md)

Rychlý prohlížeč 3D modelů a výkresů pro Windows a Linux.

## Co umí
- **3D modely STEP, IGES, STL:** strom sestavy, skrývání dílů, řezy, průhlednost
- **Měření:** vzdálenosti, průměry, úhly, tloušťky, výsledek jde zkopírovat do Excelu
- **Hmotnost a těžiště** podle materiálu, hledání kolizí mezi díly
- **Výkresy DXF a DWG:** hladiny, layouty, pravítka, měření
- **Náhledy souborů** v Průzkumníku Windows i ve správci souborů na Linuxu
- Podpora 3D myši 3Dconnexion

## Novinky ve verzi 2.0
- kompletní přepis do Rustu a GPU vykreslování (`wgpu`) bez Pythonu a Qt
- o 71 % menší instalace: Windows instalátor 16 MB (`CadProhlizec_setup_2.0.0.exe`), Linux AppImage 30,6 MB
- samostatná přenosná verze pro Windows (`StepViewer-portable-2.0.0.zip`)
- rychlejší načítání a plynulé zobrazení velkých modelů i náročných výkresů DXF/DWG

## Stažení
V sekci [Releases](../../releases/latest):
- **Windows:** `CadProhlizec_setup_2.0.0.exe` nebo přenosný `StepViewer-portable-2.0.0.zip`
- **Linux:** `cad-viewer-2.0.0-x86_64.AppImage` nebo `cad-viewer-2.0.0-linux-x86_64.tar.gz`

Ovládání programu je v češtině. Licence GPL-3.0.
