# Sicurezza

## Versioni supportate

Riceve correzioni di sicurezza solo l'ultima versione pubblicata di nextm.

## Segnalare una vulnerabilità

Non aprire una issue pubblica. Usa la segnalazione privata di GitHub: scheda **Security** del repository, pulsante
**Report a vulnerability**. Indica la versione (`nextm --version`), la versione di Windows e i passi per riprodurre
il problema. Riceverai una risposta appena possibile.

## Cosa fa nextm per limitare i rischi

- Gira senza privilegi di amministratore e senza driver.
- Non apre connessioni di rete e non ha aggiornamenti automatici.
- Carica le DLL solo dalla cartella di sistema: `SetDefaultDllDirectories` all'avvio e `/DEPENDENTLOADFLAG:0x800`
  nell'eseguibile, verificato in CI da `cargo xtask check-imports`.
- Non usa `ShellExecute`: programmi e pagine delle Impostazioni si aprono con `CreateProcessW`.
