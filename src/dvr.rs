//! DVR local: arquivos e agendamentos por aparelho, sem pressupor unidade C/J/E.
//! Gravação usa FFmpeg externo e independente da reprodução do mpv.
//! O agendador funciona com o Saimo aberto; não promete acordar um PC desligado.
use crate::{catalogo, epg, mpv};
use chrono::{Local, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

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

/// Procura uma cópia do ffmpeg distribuída junto do Saimo antes do PATH.
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

struct GravacaoAtiva {
    canal: String,
    arquivo: PathBuf,
    log: PathBuf,
    filho: Child,
    parada_solicitada: bool,
}

struct GravacaoNoPlayer {
    canal: String,
    arquivo: PathBuf,
    fim_em: Option<std::time::Instant>,
}

pub struct Dvr {
    ativos: Vec<GravacaoAtiva>,
    /// Gravação manual da fonte já reproduzida pelo mpv. Diferentemente
    /// do FFmpeg independente, acompanha o canal aberto na tela.
    no_player: Option<GravacaoNoPlayer>,
    agendados: Vec<Agendamento>,
}

impl Dvr {
    pub fn novo() -> Self {
        let agendados = fs::read_to_string(catalogo::pasta().join(AGENDA)).ok()
            .and_then(|s| serde_json::from_str::<Vec<Agendamento>>(&s).ok())
            .unwrap_or_default();
        Self { ativos: Vec::new(), no_player: None, agendados }
    }

    fn salvar_agenda(&self) {
        if let Ok(texto) = serde_json::to_string_pretty(&self.agendados) {
            let _ = fs::write(catalogo::pasta().join(AGENDA), texto);
        }
    }

    pub fn gravando(&self) -> bool { !self.ativos.is_empty() || self.no_player.is_some() }
    pub fn ha_agenda(&self) -> bool { self.agendados.iter().any(|a| !a.executado) }
    pub fn quantidade_agendada(&self) -> usize { self.agendados.iter().filter(|a| !a.executado).count() }
    pub fn ativo_no_canal(&self, nome: &str) -> bool {
        self.ativos.iter().any(|a| a.canal == nome)
            || self.no_player.as_ref().is_some_and(|r| r.canal == nome)
    }

    /// Gravação manual confiando no demuxer do próprio player, inclusive em
    /// playlists HLS disfarçadas de .txt. Não abre uma nova conexão FFmpeg.
    /// Precisa manter este canal em reprodução; não serve para agendamento
    /// independente, que continua utilizando FFmpeg.
    pub fn iniciar_no_player(
        &mut self, canal: &str, mpv: &mpv::Mpv, duracao_s: Option<u64>,
    ) -> Result<PathBuf, String> {
        if self.no_player.is_some() {
            return Err("Já existe gravação do canal em reprodução".into());
        }
        if self.ativo_no_canal(canal) {
            return Err("Este canal já está sendo gravado".into());
        }
        let arquivo = arquivo_novo(canal, "mkv", "Gravacoes")?;
        mpv.gravar_fluxo(Some(&arquivo))?;
        self.no_player = Some(GravacaoNoPlayer {
            canal: canal.into(),
            arquivo: arquivo.clone(),
            fim_em: duracao_s.map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s)),
        });
        Ok(arquivo)
    }

    pub fn parar_no_player(&mut self, mpv: &mpv::Mpv) -> Result<Option<PathBuf>, String> {
        let Some(ativo) = self.no_player.as_ref() else { return Ok(None) };
        mpv.gravar_fluxo(None)?;
        let arquivo = ativo.arquivo.clone();
        self.no_player = None;
        Ok(Some(arquivo))
    }


    pub fn iniciar(&mut self, canal: &catalogo::Canal, indice: usize, duracao_s: Option<u64>) -> Result<PathBuf, String> {
        if self.ativos.len() >= 3 { return Err("Limite de 3 gravações simultâneas.".into()); }
        if self.ativo_no_canal(&canal.nome) { return Err("Este canal já está sendo gravado.".into()); }
        let fonte = canal.fontes.get(indice).ok_or("Fonte do canal não encontrada")?;
        if fonte.chave.is_some() {
            return Err("Fonte protegida: gravação não disponível.".into());
        }
        let saida = arquivo_novo(&canal.nome, "mkv", "Gravacoes")?;
        // O erro anterior era invisível: toda saída do FFmpeg ia para null.
        // Cada gravação agora possui diagnóstico local, sem exibir URL/token na UI.
        let log = saida.with_extension("ffmpeg.log");
        let log_aberto = fs::File::create(&log)
            .map_err(|e| format!("Não foi possível criar log da gravação: {e}"))?;
        let binario = ffmpeg()?;
        let mut comando = Command::new(binario);
        comando.args(["-hide_banner", "-loglevel", "warning", "-nostats", "-y"]);
        // Uso de cabeçalhos compatíveis com o player, inclusive quando o
        // catálogo não publicou um agente personalizado.
        comando.args(["-user_agent", fonte.agente.as_deref().unwrap_or(crate::AGENTE)]);
        if let Some(referer) = fonte.referer.as_deref() {
            comando.args(["-referer", referer]);
        }
        if fonte.url.starts_with("http://") || fonte.url.starts_with("https://") {
            comando.args([
                "-reconnect", "1",
                "-reconnect_streamed", "1",
                "-reconnect_on_network_error", "1",
                "-reconnect_delay_max", "5",
            ]);
        }
        comando.arg("-i").arg(&fonte.url)
            .args(["-map", "0:v:0?", "-map", "0:a:0?", "-c", "copy"]);
        // Sem limite: grava até clicar em Parar; agendamentos sempre passam duração.
        if let Some(segundos) = duracao_s {
            comando.args(["-t", &segundos.max(1).to_string()]);
        }
        comando.args(["-f", "matroska"])
            .arg(&saida)
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::from(log_aberto));
        #[cfg(windows)] {
            use std::os::windows::process::CommandExt;
            comando.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let filho = comando.spawn().map_err(|e| format!("Não foi possível iniciar FFmpeg: {e}. Log: {}", log.display()))?;
        self.ativos.push(GravacaoAtiva {
            canal: canal.nome.clone(), arquivo: saida.clone(), log,
            filho, parada_solicitada: false,
        });
        Ok(saida)
    }

    pub fn parar(&mut self, canal: &str) -> Result<(), String> {
        let ativo = self.ativos.iter_mut().find(|a| a.canal == canal)
            .ok_or("Nenhuma gravação em andamento neste canal")?;
        if let Some(mut stdin) = ativo.filho.stdin.take() {
            // FFmpeg com -nostdin não lê 'q'; remover essa opção é essencial
            // para um encerramento limpo com índice do contêiner MKV.
            stdin.write_all(b"q\n").map_err(|e| format!("Não foi possível parar: {e}"))?;
        }
        ativo.parada_solicitada = true;
        Ok(())
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
        if !(1..=480).contains(&minutos) { return Err("Duração permitida: 1 a 480 minutos.".into()); }
        self.agendados.push(Agendamento {
            canal: canal.into(), fonte, inicio: data,
            fim: data + (minutos as i64) * 60, executado: false,
        });
        self.salvar_agenda();
        Ok(())
    }

    /// Chamar pela interface regularmente, mesmo se o usuário não estiver no canal.
    pub fn atualizar(&mut self, canais: &[catalogo::Canal], mpv: Option<&mpv::Mpv>) -> Vec<String> {
        let mut avisos = Vec::new();
        let vencida = self.no_player.as_ref()
            .and_then(|r| r.fim_em).is_some_and(|fim| std::time::Instant::now() >= fim);
        if vencida {
            if let Some(mpv) = mpv {
                match self.parar_no_player(mpv) {
                    Ok(Some(p)) => avisos.push(format!("Gravação manual finalizada: {}", p.display())),
                    Err(e) => avisos.push(format!("Não foi possível finalizar a gravação: {e}")),
                    Ok(None) => {}
                }
            }
        }
        let mut i = 0;
        while i < self.ativos.len() {
            match self.ativos[i].filho.try_wait() {
                Ok(Some(status)) => {
                    let gravacao = self.ativos.remove(i);
                    let tamanho = fs::metadata(&gravacao.arquivo).map(|m| m.len()).unwrap_or(0);
                    if status.success() && tamanho > 0 {
                        let encerramento = if gravacao.parada_solicitada { "Finalizada" } else { "Concluída" };
                        avisos.push(format!("{encerramento}: {} ({})", gravacao.canal, gravacao.arquivo.display()));
                    } else {
                        avisos.push(format!(
                            "Gravação falhou: {} (código: {:?}, arquivo: {} bytes). Log: {}",
                            gravacao.canal, status.code(), tamanho, gravacao.log.display()
                        ));
                    }
                }
                Ok(None) => i += 1,
                Err(e) => {
                    avisos.push(format!("Erro na gravação: {e}"));
                    self.ativos.remove(i);
                }
            }
        }
        let agora = epg::agora();
        let mut salvar = false;
        for i in 0..self.agendados.len() {
            if self.agendados[i].executado { continue; }
            if agora >= self.agendados[i].fim {
                self.agendados[i].executado = true;
                salvar = true;
                avisos.push(format!("Agendamento perdido (aplicativo fechado?): {}", self.agendados[i].canal));
            } else if agora >= self.agendados[i].inicio {
                let a = self.agendados[i].clone();
                self.agendados[i].executado = true;
                salvar = true;
                if let Some(c) = canais.iter().find(|c| c.nome == a.canal) {
                    match self.iniciar(c, a.fonte, Some((a.fim - agora) as u64)) {
                        Ok(p) => avisos.push(format!("Gravação agendada iniciada: {}", p.display())),
                        Err(e) => avisos.push(format!("Agendamento não iniciado: {e}")),
                    }
                } else {
                    avisos.push(format!("Canal do agendamento não encontrado: {}", a.canal));
                }
            }
        }
        if salvar { self.salvar_agenda(); }
        avisos
    }
}

// Ao fechar o Saimo, sinaliza 'q' para o FFmpeg finalizar o arquivo MKV.
// Não deixa uma gravação manual infinita invisível no sistema.
impl Drop for Dvr {
    fn drop(&mut self) {
        for gravacao in &mut self.ativos {
            if let Some(mut entrada) = gravacao.filho.stdin.take() {
                let _ = entrada.write_all(b"q\n");
            }
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
    fn pasta_padrao_nao_pressupoe_j_ou_e() {
        assert!(pasta_padrao().is_absolute() || !home().is_absolute());
    }
}
