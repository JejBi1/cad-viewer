//! Error types. Every `Display` text is the Czech message that can be shown to the user.
//! No variant ever carries a token, a code or a client secret.

use thiserror::Error;

/// Why a `cadviewer://` link was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LinkError {
    #[error("Odkaz je prázdný.")]
    Empty,
    #[error("Odkaz je příliš dlouhý.")]
    TooLong,
    #[error("Odkaz obsahuje nepovolené znaky.")]
    BadCharacters,
    #[error("Odkaz obsahuje kódované znaky (%), které nejsou povolené.")]
    PercentEncoding,
    #[error("Odkaz nepatří CAD prohlížeči (musí začínat „cadviewer://“).")]
    BadScheme,
    #[error("Odkaz obsahuje neznámý příkaz, podporovaný je pouze „open“.")]
    BadHost,
    #[error("Odkaz má nepovolenou stavbu (cesta, fragment, port nebo přihlašovací údaje).")]
    BadStructure,
    #[error("V odkazu chybí identifikátor souboru (fileId).")]
    MissingFileId,
    #[error("Odkaz obsahuje více identifikátorů souboru, najednou lze otevřít jen jeden soubor.")]
    DuplicateFileId,
    #[error("Odkaz obsahuje nepovolený parametr.")]
    UnexpectedParameter,
    #[error("Identifikátor souboru v odkazu není platný.")]
    BadFileId,
}

/// Everything that can go wrong while signing in, talking to Drive or registering the link.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DriveError {
    #[error("{0}")]
    Link(#[from] LinkError),

    #[error("Přihlášení ke Google Drive není v této verzi nastavené (chybí identifikátor klienta OAuth).")]
    NotConfigured,
    #[error("Nastavení Google Drive není platné: {0}")]
    BadConfig(String),

    #[error("Je nutné se přihlásit ke Google Drive.")]
    SignInRequired,
    #[error("Přihlášení ke Google Drive vypršelo. Zkuste to prosím znovu.")]
    SignInTimeout,
    #[error("Přihlášení bylo zrušeno.")]
    Cancelled,
    #[error("Přístup ke Google Drive nebyl povolen.")]
    AccessDenied,
    #[error("Google nepotvrdil požadované oprávnění (přístup k souborům otevřeným touto aplikací). Přihlaste se znovu a oprávnění povolte.")]
    ScopeMissing,
    #[error("Přihlášení se nezdařilo ({0}).")]
    SignInFailed(String),
    #[error("Nepodařilo se otevřít webový prohlížeč: {0}")]
    BrowserFailed(String),
    #[error("Nepodařilo se připravit přihlášení (místní příjem odpovědi): {0}")]
    Loopback(String),

    #[error("Spojení s Google Drive se nezdařilo: {0}")]
    Network(String),
    #[error("Google Drive odmítl přihlášení. Přihlaste se prosím znovu.")]
    Unauthorized,
    #[error("K tomuto souboru nemáte oprávnění nebo ho aplikace nesmí číst. Otevřete ho znovu přes „Otevřít v aplikaci“ na Google Drive.")]
    Forbidden,
    #[error("Soubor nebyl na Google Drive nalezen, nebo k němu aplikace nemá přístup. Otevřete ho znovu přes „Otevřít v aplikaci“ na Google Drive.")]
    NotFound,
    #[error("Google Drive je dočasně nedostupný nebo omezuje počet požadavků (stav {0}). Zkuste to za chvíli znovu.")]
    Unavailable(u16),
    #[error("Google Drive vrátil neočekávanou odpověď (stav {0}).")]
    HttpStatus(u16),
    #[error("Google Drive vrátil neplatnou nebo neúplnou odpověď: {0}")]
    BadResponse(String),

    #[error("Soubor „{name}“ nemá podporovanou příponu (podporované: .step, .stp, .iges, .igs, .stl, .dxf, .dwg).")]
    UnsupportedType { name: String },
    #[error("Soubor „{name}“ nelze stáhnout (je to složka nebo dokument Google, ne CAD soubor).")]
    NotDownloadable { name: String },
    #[error("Soubor „{name}“ je příliš velký ({size}), povolený limit je {limit}.")]
    TooLarge { name: String, size: String, limit: String },
    #[error("Stažený soubor je poškozený (kontrolní součet nesouhlasí). Zkuste ho otevřít znovu.")]
    ChecksumMismatch,
    #[error("Stahování bylo přerušeno.")]
    DownloadCancelled,

    #[error("Chyba při práci se souborem: {0}")]
    Io(String),
    #[error("Úložiště přihlašovacích údajů v systému selhalo: {0}")]
    CredentialStore(String),
    #[error("Registrace odkazů cadviewer:// se nezdařila: {0}")]
    Registration(String),
    #[error("Tato funkce není na tomto operačním systému podporovaná.")]
    Unsupported,
}

impl From<std::io::Error> for DriveError {
    fn from(e: std::io::Error) -> Self {
        DriveError::Io(e.to_string())
    }
}

/// Czech size text with a decimal comma, for example `12,5 MB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "kB", "MB", "GB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit]).replace('.', ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_uses_decimal_comma() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1,0 kB");
        assert_eq!(format_size(5 * 1024 * 1024 + 512 * 1024), "5,5 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3,0 GB");
        assert_eq!(format_size(5000 * 1024 * 1024 * 1024), "5000,0 GB");
    }

    #[test]
    fn messages_are_czech() {
        let e = DriveError::from(LinkError::BadFileId);
        assert_eq!(e.to_string(), "Identifikátor souboru v odkazu není platný.");
        let e = DriveError::TooLarge {
            name: "a.step".into(),
            size: "3,0 GB".into(),
            limit: "1,0 GB".into(),
        };
        assert!(e.to_string().contains("příliš velký"));
    }
}
