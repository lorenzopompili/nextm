# Changelog

Tutte le modifiche rilevanti di nextm sono annotate in questo file.

Il formato segue [Keep a Changelog](https://keepachangelog.com/it-IT/1.1.0/) e il progetto usa il
[versionamento semantico](https://semver.org/lang/it/).

## [Non rilasciato]

### Aggiunto (milestone M4: Finestra Unificata di Ispezione, Impostazioni integrate, Installer Inno Setup e Ottimizzazioni GDI)

- **Finestra Unificata di Ispezione di Sistema ("Il Grande Passo")**:
  - Apertura istantanea al clic sinistro o doppio clic su qualsiasi icona di `nextm` nella barra delle applicazioni.
  - Interfaccia fluida nativa con doppio buffer GDI senza sfarfallio, angoli arrotondati DWM e supporto dark/light mode.
  - **Scheda Processi (Task Manager)**:
    - Scansione ad altissima efficienza via `NtQuerySystemInformation(SystemProcessInformation)`.
    - Modalità raggruppata ad albero (per nome eseguibile) o visualizzazione piatta dei singoli PID.
    - Espansione/chiusura gruppi tramite tastiera (frecce destra/sinistra), clic su spunta o doppio clic.
    - Metriche complete per processo: CPU %, Working Set, Memoria Privata (commit), Thread, numero Socket e riepilogo Servizi ospitati.
    - Ricerca e filtro in tempo reale zero-allocazioni ASCII (per nome processo, PID o servizi).
    - Ordinamento su tutte le colonne (ascendente/discendente).
    - Terminazione forzata sicura del processo selezionato (`kill_process`).
  - **Scheda Connessioni di Rete (stile TCPView)**:
    - Scansione estesa tabelle TCP e UDP (IPv4 e IPv6) via `iphlpapi.dll` dinamica (zero import statici).
    - Correlazione automatica O(1) a processo host, PID, porte locali/remote, protocollo e stato TCP.
    - Raggruppamento per processo e chiusura forzata connessioni TCP IPv4 attive.
  - **Scheda Servizi Windows**:
    - Scansione del Service Control Manager (`advapi32.dll`) con associazione diretta ai processi host (es. `svchost.exe`).
    - Visualizzazione stato servizio (In esecuzione, Arrestato, In avvio, ecc.).
    - Azioni di avvio (`start_service`) e arresto (`stop_service`) dei servizi selezionati.
  - **Scheda Impostazioni Applicazione**:
    - Spostamento di tutta la configurazione prima dispersa nel menu contestuale destro in un'interfaccia a schede moderne ordinate a 3 card:
      - **Metriche & Icone**: attivazione singola o multipla di CPU, RAM, Rete, ACPI, GPU, Disco; scelta stile icone (Solo numeri ad alta leggibilità vs Icona pixel-art tematica + numero); formato rete (Bit/s vs Byte/s); selezione lingua interfaccia (**Auto OS**, **Italiano**, **English**) con cambio dinamico a runtime.
      - **Calcolo & Prestazioni**: modalità CPU (Standard vs Utilità PDH); dettaglio core logici al passaggio del mouse; monitoraggio core saturo; intervallo di campionamento (1s, 2s, 5s); modalità risparmio energetico EcoQoS.
      - **Avvio Automatico & Privilegi**: avvio all'accesso utente (registro `Run`); avvio con privilegi massimi via Utilità di Pianificazione (`schtasks /rl highest`); pulsante "🛡️ Riavvia come Amministratore" on-demand; collegamento diretto alle impostazioni barra di Windows 11; indicatore storage impostazioni (registro HKCU vs portable `nextm.ini`).
  - **Scheda Informazioni**:
    - Logo vettoriale renderizzato ad alta risoluzione (96x96 px), badge privilegi (Admin vs Utente standard), attribuzione autore (**Lorenzo Pompili**), specifiche architetturali e licenze.
- **Riorganizzazione Menu Contestuale Destro**:
  - Menu destro snellito, essenziale ed elegante (Apri ispezione / Impostazioni, Esci).
- **Gestione Privilegi Elevati e UAC Restart**:
  - Funzione `restart_as_admin` affidabile: release atomico del mutex di istanza singola prima della chiusura del processo genitore, consentendo alla nuova istanza elevata di avviarsi istantaneamente senza conflitti o blocchi.
  - Abilitazione automatica di `SeDebugPrivilege` nel token del processo se avviato con privilegi di amministratore.
  - Integrazione con l'Utilità di Pianificazione di Windows per avvio automatico all'accesso con privilegi massimi (`/rl highest`) senza prompt UAC all'accensione del PC.
- **Installer Windows Inno Setup Bilingue & Uninstaller a Pulizia Millimetrica**:
  - Dialogo iniziale di selezione lingua (Italiano / English) con messaggi personalizzati localizzati.
  - Generazione automatizzata dell'installer con `cargo xtask dist` (`nextm-setup-v0.1.0.exe`).
  - Modalità user-mode predefinita (senza UAC in `%LocalAppData%\Programs\nextm`) con override facoltativo per tutti gli utenti (`Program Files`).
  - Selezione opzionale di icona sul desktop e nel menu Start di Windows.
  - Task di installazione per avvio automatico con Windows.
  - Disinstallatore con pulizia al 100%: arresto dei processi attivi via `taskkill`, rimozione attività pianificata via `schtasks`, cancellazione file portable locali (`nextm.ini`), cartelle e rimozione completa delle chiavi di registro utente `HKCU\Software\nextm`.
- **Ottimizzazioni Prestazionali & Correzioni Win32 Handle**:
  - **GDI Font Caching**: introdotte le strutture RAII `InspectFonts` e `HoverFonts` che mantengono in cache tutti i font GDI (`HFONT`) per la finestra di ispezione e per il riquadro hover, azzerando le continue chiamate `CreateFontW`/`DeleteObject` a ogni movimento del mouse o frame di repaint.
  - **Supporto Per-Monitor DPI V2**: rilevamento del DPI nativo all'apertura (`dpi_of(hwnd)`) e gestione del messaggio `WM_DPICHANGED` (`0x02E0`) con ridimensionamento finestra, ri-rasterizzazione font e aggiornamento font della casella di ricerca (`WM_SETFONT`).
  - **Risolto Double-Close su Mutex**: corretto `SingleInstance::drop` con swap atomico su puntatore nullo prima di `CloseHandle`.
  - **Bilanciamento handle DLL**: bilanciate le chiamate `LoadLibraryExW` con `FreeLibrary` in `darkmenu.rs`.
  - **Confronti ASCII Zero-Allocazioni**: implementati `cmp_ignore_ascii_case`, `contains_ignore_ascii_case` e `pid_contains`.
  - **Eliminato import statico `bcryptprimitives.dll`**: migrata la correlazione interna di `InspectEngine` da `HashMap` standard a `BTreeMap`.

### Aggiunto (milestone M3: temperature ACPI, GPU, disco e dettaglio CPU per-core al passaggio del mouse)

- **Temperature hardware a zero driver e zero privilegi di amministratore**:
  - **CPU / Scheda madre (ACPI)**: lettura delle zone termiche ACPI tramite PerfLib V2 (`advapi32.dll` caricata solo su attivazione). Rilevamento automatico dei sensori fissi/fittizi e riconoscimento della temperatura CPU dall'Embedded Controller su hardware validato (es. Lenovo).
  - **GPU**: lettura della temperatura della scheda grafica (discreta o integrata) tramite interfaccia kernel DirectX D3DKMT (`D3DKMTQueryAdapterInfo` query 62 `D3DKMT_ADAPTER_PERFDATA` su `gdi32.dll` on-demand). Cadenza lenta a 5 secondi e selezione GPU multi-scheda via menu.
  - **Disco / SSD (NVMe / SATA)**: lettura temperatura e classificazione del supporto ("SSD NVMe", "SSD" o "Disco") tramite `IOCTL_STORAGE_QUERY_PROPERTY` (proprietà 52) con handle senza privilegi (`FILE_READ_ATTRIBUTES` / accesso 0). Cadenza lenta a 60 secondi.
- **Finestra hover ricca ("riquadro") al passaggio del mouse**:
  - Finestra nativa senza attivazione di focus (`WS_EX_NOACTIVATE`), angoli arrotondati DWM e tema scuro/chiaro automatico, doppio buffer GDI senza sfarfallio.
  - Caratteri compatti a 9pt uniformati alla scala del dettaglio core (`C0: 12% ...`) e dimensioni del riquadro snellite.
  - Mostra istantaneamente e simultaneamente **tutte** le metriche attive: CPU, processo saturo, RAM (con GB usati/totali), Rete (download/upload e nome scheda) e Temperature (CPU ACPI, GPU con modello, Disco SSD).
  - Soppressione del tooltip standard di Explorer: appare esclusivamente il nuovo riquadro ricco senza doppie finestre sovrapposte.
- **Dettaglio CPU singoli core verticale e continuo**:
  - Campionamento fisso e continuo quando l'opzione è attiva (senza decadere o resettarsi dopo pochi secondi).
  - Visualizzazione a griglia verticale su più colonne (es. 3 colonne da 8 righe per CPU da 24 core), con font monospazio e colorazione in ambra per core con carico >= 90%.
- **Icone della tray con mini-icone tematiche (stile FontAwesome)**:
  - Cifre numeriche compatte ad alta definizione (`FontKind::Small`) posizionate nella metà inferiore dell'icona.
  - Mini-icona pixel-art dedicata nella metà superiore per riconoscere ogni metrica a colpo d'occhio:
    - **CPU**: microchip con pin e die centrale (`CPU_SYMBOL`, stile `fa-microchip`), con eventuale barra di saturazione in basso.
    - **RAM**: modulo di memoria DIMM con chip e tacca di inserzione (`RAM_SYMBOL`, stile `fa-memory`).
    - **Temp CPU / ACPI**: termometro con bulbo e colonna graduata (`TEMP_ACPI_SYMBOL`, stile `fa-temperature-half`).
    - **Temp GPU**: scheda video con doppia ventola di dissipazione e slot PCIe (`TEMP_GPU_SYMBOL`).
    - **Temp Disco / SSD**: involucro storage con piastra e LED di attività (`TEMP_DISK_SYMBOL`, stile `fa-hdd`).
- **Menu contestuale persistente per selezioni multiple**:
  - Cliccando su spunte di attivazione metriche, icone o opzioni, il menu resta aperto e aggiorna lo stato visivo delle voci in tempo reale, evitando di doverlo riaprire ripetutamente.
- **Schema impostazioni v3**: memorizzazione di `CpuPerCoreHover`, `GpuLuid`, e bitmask `METRIC_TEMP_*` e `ICON_TEMP_*`.

### Aggiunto (milestone M2: RAM, rete, utilità CPU, core saturo, icone multiple)

- Icona della **RAM** nella tray: percentuale di memoria fisica usata, livello di allarme (ambra dal 70%, rosso dal 90% con isteresi), memoria usata e totale in GiB nel tooltip.
- Icona della **Rete** nella tray: velocità di download e upload su due righe compatte con unità automatiche (B/s, K/s, M/s, G/s o b/s, k/s, m/s, g/s); selezione interfaccia (somma, automatica o specifica da menu con elenco dinamico delle schede attive da `GetIfTable2`).
- Misura dell'**Utilità CPU** opzionale tramite contatori di sistema/PDH (`Processor Utility`), allineata al 100% di Task Manager con frequenza nominale o turbo.
- Rilevamento del **Core Saturo**: monitoraggio dei singoli thread tramite scansione efficiente `NtQuerySystemInformation(SystemProcessInformation)` con allocazione e liberazione su soglia (`gate_percent` = 100 / N core). Mostra una barra di stato sotto la percentuale CPU (da 1 a 4 segmenti) e indica il processo responsabile nel tooltip. Modalità configurabile: Sempre, Al passaggio del mouse, Spento.
- Gestione di **icone multiple** nella tray (CPU, RAM, Rete attivabili singolarmente o in combinazione) e icona neutra statica (tachimetro) se tutte le metriche sono spente ma nextm resta in esecuzione.
- Tooltip composito multi-riga con tutte le metriche attive in italiano e inglese.
- Schema impostazioni v2 con bitmask `Metrics` e `Icons`, selezione interfaccia di rete persistente tramite LUID, unità bit/byte, modalità CPU e saturazione.

### Aggiunto (milestone M1: fondamenta e icona della CPU)

- Icona della **CPU** nella tray, con cifre a pixel disegnate a mano per 16, 20, 24, 28 e 32 px e raddoppiate oltre
  il 200% di scala; colore per livello (ambra dal 70%, rosso dal 90% con isteresi); tema chiaro, scuro e alto
  contrasto.
- Lettura della CPU con `SystemPerformanceInformation`: stessi valori di `GetSystemTimes` (scarto medio 0,16 punti)
  con circa 8 volte meno cambi di contesto, senza far girare il thread su tutti i core. Supporta più gruppi di
  processori; ripiego automatico sulla lettura per processore.
- Media degli ultimi 3 campioni e isteresi di 1 punto sul numero mostrato: l'icona non si ridisegna per il rumore.
- Tooltip aggiornato insieme all'icona e al passaggio del mouse.
- Menu: intervallo (1/2/5 s), risparmio energia (pausa a schermo spento, più lento col risparmio energia, EcoQoS),
  avvio con Windows, "Mostra sempre l'icona…", informazioni, esci. Menu scuro con il tema scuro.
- Impostazioni nel registro (`HKCU\Software\nextm`) oppure, in **modalità portable**, nel file `nextm.ini` accanto
  all'eseguibile.
- Notifica al primo avvio (riproposta al massimo 2 volte finché non viene cliccata) che apre le Impostazioni per
  rendere visibile l'icona.
- Istanza singola; una seconda istanza fa comparire una notifica nella prima.
- Riga di comando: `--version`, `--diagnose`, `--quit`.
- Robustezza: riavvio di Explorer, cambio di DPI e di tema, sospensione e ripresa, schermo spento.
- Strumento `xtask`: controllo degli import dell'eseguibile (costo zero, niente DLL caricate da altre cartelle),
  tetto "a cricchetto" sulla dimensione, `bench` delle risorse usate.
- CI su GitHub Actions per x64 e ARM64.
- Documentazione: README, CONTRIBUTING, SECURITY, licenze MIT e Apache 2.0.
