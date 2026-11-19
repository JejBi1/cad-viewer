//! Czech texts of the window that are longer than a label: the start screen, the help and the about box. The
//! `**` markers in the texts are bold (see `rich::layout`).

pub const APP_NAME: &str = "CAD prohlížeč";
/// Version shown in the about box and used as the cache version (`2.0`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const EMPTY_STATE: &str = "Otevři model nebo výkres (Ctrl+O), nebo ho sem přetáhni.\n\n3D: STEP, IGES, STL   ·   2D: DXF, DWG";

/// Paragraphs of the start screen in the properties panel (`show_welcome`).
pub const WELCOME: [&str; 6] = [
    "3D: **STEP, IGES, STL** · 2D: **DXF, DWG**",
    "Otevři soubor **Ctrl+O**, nebo ho přetáhni do okna.",
    "**3D myš (jako Onshape):** pravé tlačítko + tažení = otáčení kolem bodu pod kurzorem, prostřední nebo Ctrl+pravé = posun, kolečko = zoom, šipky = otočení o 15°. Klik levým = výběr, Ctrl+klik = přidat k výběru, dvojklik = přiblížit.",
    "**2D myš:** tažení = posun, kolečko = zoom, klik = údaje o entitě (délka, poloměr, kóta…).",
    "**3Dconnexion SpaceMouse:** model se pohybuje jako čepička – posun, tah k sobě = přiblížit, kroucení a naklápění = otáčení (kolem bodu uprostřed obrazovky); tlačítko Fit = vše, na SpaceMouse Pro i pohledy. Nastavení rychlosti a os: šipka u „Otevřít“ › 3D myš.",
    "**Klávesy:** F vše · Z na výběr · H skrýt · I jen vybrané · A vše zobrazit · T průhledné · E hrany · W drátově · P perspektiva · M měřit (vrcholy, hrany, plochy) · G hmotnost a těžiště · K kolize · 0–6 nebo Shift+1…7 pohledy · F5 znovu načíst · F1 nápověda",
];

pub const ABOUT_DESCRIPTION: &str = "Prohlížení STEP / IGES / STL modelů a DXF / DWG výkresů.\nJádro Open CASCADE (OCCT), vykreslování wgpu, rozhraní egui.";

/// Paragraphs of the about box below the heading.
pub const ABOUT: [&str; 4] = [
    "**Myš (jako Onshape):** pravé + tažení = otáčení kolem bodu pod kurzorem,\nprostřední nebo Ctrl+pravé = posun, kolečko = zoom, šipky = 15°, Shift+šipky = 90°.\nKlik = výběr, Ctrl+klik = přidat, dvojklik = přiblížit, pravé (klik) = menu.\nKlik na kostku vpravo nahoře = natočení pohledu.",
    "**Měřit (M):** klikej na vrcholy, hrany a plochy (opakovaný klik odebere, klik do prázdna vyčistí). 1 entita = délka, Ø / R, úhel oblouku, plocha, normála / osa. 2 entity = nejmenší vzdálenost + ΔX / ΔY / ΔZ v globálních souřadnicích (barevně X / Y / Z), vzdálenost středů a os, vzdálenost rovin, úhel. Více = součet délek / ploch.\n**Materiál…** (pravé menu) přiřadí hustotu dílům (pamatuje se k souboru), **Hmotnost (G)** = hmotnost, objem a těžiště výběru / modelu, **Kolize (K)** = průniky mezi díly (vybranými, jinak všemi viditelnými).",
    "**Klávesy:**\nF = celý model, Z = na výběr, pohledy 0–6 nebo jako Onshape Shift+1 přední, Shift+2 zadní, Shift+3 levý, Shift+4 pravý, Shift+5 horní, Shift+6 spodní, Shift+7 izometrie\nH = skrýt, I = jen vybrané, A = zobrazit vše, T = průhledné\nE = hrany, W = drátově, P = perspektiva, M = měřit, U = úhel 3 body, G = hmotnost, K = kolize, Esc = zrušit\nCtrl+O = otevřít, F5 = znovu načíst, Ctrl+S = snímek",
    "Rozměry jsou obalový kvádr v souřadnicích modelu (mm). Bez přiřazeného materiálu se ukazuje jen odhad hmotnosti pro ocel / hliník.",
];

pub const MEASURE_HINT_3D: &str = "Měřit: klikej na vrcholy, hrany a plochy (další klik přidá, opakovaný odebere, prázdno = vyčistit), Shift+klik = bod na ploše, Enter = ponechat, Ctrl+C = kopírovat, Esc = konec";
pub const MEASURE_HINT_2D: &str = "Vzdálenost: klikni na 2 body – přichytí se k vrcholům / bodům geometrie, Shift = vodorovně / svisle.  Měření zůstávají, Esc = smazat a konec";
pub const ANGLE_HINT: &str = "Úhel: klikni na konec 1. ramene, vrchol a konec 2. ramene.  Esc = smazat a konec";

/// Help text of the measure tool in the properties panel while nothing is picked (2152-2161).
pub const MEASURE_HELP: [&str; 2] = [
    "Klikni na vrchol, hranu nebo plochu. Druhý klik přidá další entitu – ukáže se vzdálenost (nejmenší i ΔX/ΔY/ΔZ v globálních souřadnicích), vzdálenost středů / os, tloušťka a úhel. Více ploch / hran = součet ploch / délek.",
    "Shift+klik = přesný bod na ploše (s normálou). Enter = ponechat měření a měřit dál, Ctrl+C = zkopírovat výsledek, Esc = konec.",
];
pub const MEASURE_HELP_STL: &str = "klik na síť = bod (přichytí se k vrcholu, modrá tečka); na rovinné ploše ukáže i její plochu v mm². Klik na ostrou hranu = obrys, okraj otvoru dá Ø a střed. Dva body na protilehlých stěnách = tloušťka. Shift+klik = jen bod (bez přichycení k hraně).";
pub const MEASURE_FOOTER: &str = "Enter = ponechat · Ctrl+C = kopírovat";
/// Value of a measurement or a size row while the interference check owns the document: it comes back when the check
/// has finished and the panel is built again.
pub const COMPUTING_AFTER_CHECK: &str = "Hodnota se počítá, až skončí kontrola kolizí…";

/// Short note of the status bar for a drawing that is drawn with less detail (the notes of the scene, which the panel of the
/// drawing shows in full above everything else).
pub const STATUS_REDUCED_DETAIL: &str = "Výkres zjednodušen – viz Upozornění";
/// Tooltip of that note.
pub const STATUS_REDUCED_DETAIL_TIP: &str = "Výkres je velmi rozsáhlý a pro úsporu paměti je vykreslen s menším detailem. Podrobnosti jsou v panelu vlevo dole (Upozornění).";

pub const COG_STATUS: &str = "Těžiště zobrazeno modrou tečkou (T). Esc nebo nový výběr = skrýt";
pub const COPIED: &str = "Výsledek měření zkopírován do schránky";

pub const OPEN_TITLE: &str = "Otevřít model / výkres";
pub const SAVE_TITLE: &str = "Uložit snímek";
pub const ALL_FILTER: &str = "Všechny CAD soubory";
pub const MODEL_FILTER: &str = "3D modely";
pub const DRAWING_FILTER: &str = "Výkresy";
pub const EVERY_FILTER: &str = "Všechny soubory";

/// The entries of the menu behind the arrow of "Otevřít" for the Google Drive link. The help page of the launcher
/// (`cad-drive-web/index.html`) names the first one; a test keeps the two in step.
pub const MENU_REGISTER_LINK: &str = "Zaregistrovat odkazy Google Drive";
pub const MENU_UNREGISTER_LINK: &str = "Odregistrovat odkazy Google Drive";
pub const MENU_DRIVE_SIGN_OUT: &str = "Odhlásit se z Google Drive";
pub const TIP_REGISTER_LINK: &str = "Zapíše odkazy cadviewer:// jen pro tvůj účet (HKCU, bez práv správce), aby „Otevřít v aplikaci“ na Google Drive spustilo tento program";
pub const TIP_UNREGISTER_LINK: &str = "Odstraní odkazy cadviewer:// zapsané tímto programem";
pub const TIP_DRIVE_SIGN_OUT: &str = "Zapomene přihlášení ke Google Drive na tomto počítači; příště se přihlásíš znovu";
/// Result of the sign-out entry.
pub const SIGNED_OUT: &str = "Přihlášení ke Google Drive bylo na tomto počítači zapomenuto. Při příštím otevření souboru z Google Drive se přihlásíš znovu. Přístup, který jsi už Googlu povolil, jde zrušit v nastavení účtu Google.";
pub const NOTHING_TO_SIGN_OUT: &str = "V této verzi programu není přihlášení ke Google Drive zapnuté, není se z čeho odhlásit.";

/// The Google Drive link cannot be opened because this build has no OAuth client.
pub const DRIVE_NOT_CONFIGURED: &str = "Otevření souboru z Google Drive není v této verzi programu zapnuté: sestavení nemá nastavený identifikátor klienta OAuth (CADVIEWER_DRIVE_CLIENT_ID). Soubor si stáhni z Google Drive ručně a otevři ho přes Ctrl+O, nebo použij verzi programu s přihlášením ke Google Drive.";
