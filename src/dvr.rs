//! DVR local: arquivos e agendamentos por aparelho, sem pressupor unidade C/J/E.
//! Gravação usa FFmpeg externo e independente da reprodução do mpv.
//! O agendador funciona com o Saimo aberto; não promete acordar um PC desligado.
use crate::{catalogo, epg, mpv};
use chrono::{Local, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CONFIG: &str = "gravacoes-config.json";
const AGENDA: &str = "gravacoes-agendadas.json";

#[derive(Default, Serialize, Deserialize)]
struct Config {
    pasta: Option<PathBuf>,
}

fn home() -> PathBuf {
    #[cfg(windows)]
    { return std::env::var_os("USERPROFILE").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".")); }
    #[cfg(not(windows))]
    { std::env::var_os("HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".")) }
}

/// Pasta de vídeo do usuário, em vez de assumir C:, J: ou E:.
pub fn pasta_padrao() -> PathBuf {
    // Instalações portáteis e testes podem fornecer outro volume sem alterar
    // a preferência persistida. Ex.: SAIMO_GRAVACOES_DIR=J:\\SaimoTV\\Gravacoes.
    if let Some(dir) = std::env::var_os("SAIMO_GRAVACOES_DIR")
        .map(PathBuf::from).filter(|p| p.is_absolute()) {
        return dir;
    }
    #[cfg(target_os = "windows")]
    { home().join("Videos").join("Saimo TV") }
    #[cfg(target_os = "macos")]
    { home().join("Movies").join("Saimo TV") }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let base = std::env::var("XDG_VIDEOS_DIR").ok()
            .map(|s| s.replace("$HOME", &home().to_string_lossy()))
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home().join("Videos"));
        base.join("Saimo TV")
    }
}

pub fn pasta() -> PathBuf {
    fs::read_to_string(catalogo::pasta().join(CONFIG)).ok()
        .and_then(|s| serde_json::from_str::<Config>(&s).ok())
        .and_then(|c| c.pasta)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(pasta_padrao)
}

pub fn configurar_pasta(texto: &str) -> Result<PathBuf, String> {
    let destino = if texto.trim().is_empty() {
        pasta_padrao()
    } else {
        PathBuf::from(texto.trim().trim_matches('"'))
    };
    if !destino.is_absolute() {
        return Err("Informe um caminho absoluto, como D:\\\\Gravacoes ou /home/usuario/Videos".into());
    }
    fs::create_dir_all(&destino).map_err(|e| format!("Pasta inacessível: {e}"))?;
    // Confirmar permissão de escrita antes de salvar a preferência.
    let teste = destino.join(format!(".saimo-verificacao-{}", std::process::id()));
    fs::write(&teste, b"ok").map_err(|e| format!("Sem permissão para gravar nesta pasta: {e}"))?;
    let _ = fs::remove_file(teste);
    let json = serde_json::to_string_pretty(&Config { pasta: Some(destino.clone()) })
        .map_err(|e| e.to_string())?;
    fs::write(catalogo::pasta().join(CONFIG), json).map_err(|e| e.to_string())?;
    Ok(destino)
}

fn nome_seguro(nome: &str) -> String {
    let limpo: String = nome.chars().map(|c| {
        if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' }
    }).take(70).collect();
    let limpo = limpo.trim().trim_matches('.').to_string();
    if limpo.is_empty() { "Canal".into() } else { limpo }
}

fn arquivo_novo(canal: &str, extensao: &str, subpasta: &str) -> Result<PathBuf, String> {
    let destino = pasta().join(subpasta);
    fs::create_dir_all(&destino).map_err(|e| format!("Não foi possível criar a pasta: {e}"))?;
    for n in 0..1000 {
        let data = Local::now().format("%Y-%m-%d_%H-%M-%S");
        let sufixo = if n == 0 { String::new() } else { format!("-{n}") };
        let arquivo = destino.join(format!("{}_{}{}.{}", nome_seguro(canal), data, sufixo, extensao));
        if !arquivo.exists() { return Ok(arquivo); }
    }
    Err("Não foi possível gerar nome exclusivo para o arquivo".into())
}

pub fn captura(mpv: &mpv::Mpv, canal: &str) -> Result<PathBuf, String> {
    let arquivo = arquivo_novo(canal, "png", "Capturas")?;
    mpv.capturar_para(&arquivo)?;
    Ok(arquivo)
}

/// Procura o ffmpeg portátil distribuído junto do Saimo; sem exigir OBS.
fn ffmpeg() -> Result<PathBuf, String> {
    let nome = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Some(p) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(nome))) {
        if p.is_file() { return Ok(p); }
    }
    if let Some(dir) = std::env::var_os("SAIMO_FFMPEG") {
        let p = PathBuf::from(dir);
        if p.is_file() { return Ok(p); }
    }
    let disponivel = Command::new(nome).arg("-version")
        .stdout(Stdio::null()).stderr(Stdio::null()).status()
        .map(|s| s.success()).unwrap_or(false);
    if disponivel { Ok(PathBuf::from(nome)) }
    else { Err("FFmpeg não encontrado. Coloque ffmpeg ao lado do executável ou instale-o no PATH.".into()) }
}


#[derive(Clone, Serialize, Deserialize)]
pub struct Agendamento {
    pub canal: String,
    pub fonte: usize,
    pub inicio: i64,
    pub fim: i64,
    #[serde(default)]
    pub executado: bool,
}

/// O DVR nunca recodifica vídeo nem áudio; cada sessão contém partes
/// independentes. Não precisa manter a janela Saimo visível para continuar.
pub struct Dvr {
    ativos: Vec<crate::dvr_stream::GravacaoStream>,
    agendados: Vec<Agendamento>,
    avisos_iniciais: Vec<String>,
}

impl Dvr {
    pub fn novo() -> Self {
        let agendados = fs::read_to_string(catalogo::pasta().join(AGENDA)).ok()
            .and_then(|s| serde_json::from_str::<Vec<Agendamento>>(&s).ok())
            .unwrap_or_default();
        let mut avisos_iniciais = Vec::new();
        if let Ok(binario) = ffmpeg() {
            let recuperadas = crate::dvr_stream::recuperar_sessoes(
                &pasta().join("Gravacoes"), &binario
            );
            if recuperadas > 0 {
                avisos_iniciais.push(format!(
                    "DVR recuperando {recuperadas} sessão(ões) interrompida(s). Os trechos permanecem salvos."
                ));
            }
        }
        Self { ativos: Vec::new(), agendados, avisos_iniciais }
    }

    fn salvar_agenda(&self) {
        if let Ok(texto) = serde_json::to_string_pretty(&self.agendados) {
            let temp = catalogo::pasta().join("gravacoes-agendadas.tmp");
            if fs::write(&temp, texto).is_ok() {
                let _ = fs::rename(temp, catalogo::pasta().join(AGENDA));
            }
        }
    }

    pub fn gravando(&self) -> bool { !self.ativos.is_empty() }
    pub fn ha_agenda(&self) -> bool { self.agendados.iter().any(|a| !a.executado) }
    pub fn quantidade_agendada(&self) -> usize {
        self.agendados.iter().filter(|a| !a.executado).count()
    }
    pub fn ativo_no_canal(&self, nome: &str) -> bool {
        self.ativos.iter().any(|r| r.canal == nome)
    }

    pub fn iniciar(&mut self, canal: &catalogo::Canal, indice: usize, duracao: Option<u64>) -> Result<PathBuf, String> {
        if self.ativos.len() >= 3 { return Err("Limite de três gravações simultâneas".into()); }
        if self.ativo_no_canal(&canal.nome) {
            return Err("Este canal já está sendo gravado".into());
        }
        let ffmpeg = ffmpeg()?;
        let rec = crate::dvr_stream::GravacaoStream::iniciar(
            canal, indice, duracao, ffmpeg, &pasta().join("Gravacoes")
        )?;
        let pasta = rec.pasta.clone();
        self.ativos.push(rec);
        Ok(pasta)
    }

    pub fn parar(&mut self, canal: &str) -> Result<(), String> {
        let ativo = self.ativos.iter_mut().find(|a| a.canal == canal)
            .ok_or("Nenhuma gravação em andamento neste canal")?;
        ativo.solicitar_parada()
    }

    pub fn agendar(&mut self, canal: &str, fonte: usize, inicio: &str, minutos: u32) -> Result<(), String> {
        let formato = NaiveDateTime::parse_from_str(inicio.trim(), "%Y-%m-%d %H:%M")
            .map_err(|_| "Data inválida. Use AAAA-MM-DD HH:MM".to_string())?;
        let data = Local.from_local_datetime(&formato).single()
            .ok_or("Horário inexistente ou ambíguo no fuso local")?.timestamp();
        let agora = epg::agora();
        if data < agora || data > agora + 90 * 24 * 3600 {
            return Err("Escolha uma data futura dentro de 90 dias.".into());
        }
        if !(1..=480).contains(&minutos) {
            return Err("Duração permitida: 1 a 480 minutos.".into());
        }
        self.agendados.push(Agendamento {
            canal: canal.into(), fonte, inicio: data,
            fim: data + (minutos as i64) * 60, executado: false
        });
        self.salvar_agenda();
        Ok(())
    }

    /// Roda a cada segundo, sem bloquear a interface.
    pub fn atualizar(&mut self, canais: &[catalogo::Canal]) -> Vec<String> {
        let mut avisos = std::mem::take(&mut self.avisos_iniciais);
        let mut i = 0;
        while i < self.ativos.len() {
            let (ativa, mensagens) = self.ativos[i].atualizar();
            avisos.extend(mensagens);
            if ativa { i += 1; } else { self.ativos.remove(i); }
        }

        let agora = epg::agora();
        let mut salvar = false;
        for i in 0..self.agendados.len() {
            if self.agendados[i].executado { continue; }
            let a = self.agendados[i].clone();
            if agora >= a.fim {
                self.agendados[i].executado = true;
                salvar = true;
                avisos.push(format!(
                    "Gravação programada perdida (Saimo fechado/desligado?): {}", a.canal
                ));
            } else if agora >= a.inicio {
                // Marcar como iniciada antes de criar o processo impede disparo
                // duplicado no mesmo instante; sessões parcialmente gravadas
                // continuam disponíveis mesmo após encerramento inesperado.
                self.agendados[i].executado = true;
                salvar = true;
                if let Some(c) = canais.iter().find(|c| c.nome == a.canal) {
                    match self.iniciar(c, a.fonte, Some((a.fim - agora) as u64)) {
                        Ok(p) => avisos.push(format!(
                            "DVR: agendamento iniciado para {}. Verificando dados em {}",
                            c.nome, p.display()
                        )),
                        Err(e) => avisos.push(format!(
                            "DVR: agendamento de {} falhou: {e}", a.canal
                        )),
                    }
                } else {
                    avisos.push(format!(
                        "DVR: canal agendado não encontrado no catálogo: {}", a.canal
                    ));
                }
            }
        }
        if salvar { self.salvar_agenda(); }
        avisos
    }
}

/// Não destruir os arquivos se o usuário fechar o Saimo durante a gravação.
impl Drop for Dvr {
    fn drop(&mut self) {
        for rec in &mut self.ativos {
            let _ = rec.solicitar_parada();
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn sanitiza_nome_para_arquivos() {
        assert_eq!(nome_seguro("SBT: São Paulo / HD"), "SBT_ São Paulo _ HD");
    }

    #[test]
    fn pasta_padrao_por_dispositivo() {
        assert!(pasta_padrao().is_absolute() || !home().is_absolute());
    }
}
