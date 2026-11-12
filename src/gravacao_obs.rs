//! Controle local do OBS Studio para captura do vídeo realmente exibido e do
//! áudio do Saimo. Requer uma cena OBS configurada uma vez pelo usuário.
//! Nenhuma senha é persistida; somente WebSocket em 127.0.0.1:4455.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tungstenite::{connect, stream::MaybeTlsStream, Message, WebSocket};
use std::net::TcpStream;

pub struct Resultado {
    pub gravando: bool,
    pub mensagem: String,
}

fn assinatura(senha: &str, sal: &str, desafio: &str) -> String {
    let segredo = STANDARD.encode(Sha256::digest(format!("{senha}{sal}").as_bytes()));
    STANDARD.encode(Sha256::digest(format!("{segredo}{desafio}").as_bytes()))
}

fn receber(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Result<Value, String> {
    loop {
        let msg = ws.read().map_err(|e| format!("OBS desconectado: {e}"))?;
        if msg.is_ping() || msg.is_pong() { continue; }
        if msg.is_close() { return Err("OBS encerrou a conexão WebSocket".into()); }
        let texto = msg.into_text().map_err(|e| format!("Resposta OBS inválida: {e}"))?;
        return serde_json::from_str(&texto).map_err(|e| format!("JSON do OBS inválido: {e}"));
    }
}

fn enviar(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, msg: Value) -> Result<(), String> {
    ws.send(Message::text(msg.to_string()))
        .map_err(|e| format!("Falha ao enviar comando ao OBS: {e}"))
}

fn chamar(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    tipo: &str,
    serial: &mut usize,
) -> Result<Value, String> {
    *serial += 1;
    let id = format!("saimo-{}", serial);
    enviar(ws, json!({
        "op": 6,
        "d": { "requestType": tipo, "requestId": id }
    }))?;
    loop {
        let resposta = receber(ws)?;
        if resposta["op"] == 7 && resposta["d"]["requestId"].as_str() == Some(id.as_str()) {
            let dado = &resposta["d"];
            if dado["requestStatus"]["result"] != true {
                let detalhe = dado["requestStatus"]["comment"].as_str().unwrap_or("sem detalhes");
                return Err(format!("OBS recusou {tipo}: {detalhe}"));
            }
            return Ok(dado["responseData"].clone());
        }
    }
}

pub fn controlar(gravar: bool, cena: &str, senha: &str) -> Result<Resultado, String> {
    if cena.trim().is_empty() { return Err("Informe o nome da cena OBS".into()); }
    let (mut ws, _) = connect("ws://127.0.0.1:4455").map_err(|_| {
        "OBS não conectado. Abra o OBS Studio e habilite Ferramentas > Servidor WebSocket (porta 4455).".to_string()
    })?;
    if let MaybeTlsStream::Plain(tcp) = ws.get_mut() {
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(7)));
        let _ = tcp.set_write_timeout(Some(Duration::from_secs(7)));
    }
    let hello = receber(&mut ws)?;
    if hello["op"] != 0 {
        return Err("O serviço na porta 4455 não é um OBS WebSocket compatível".into());
    }
    let senha_obs = &hello["d"]["authentication"];
    let mut identificar = json!({"rpcVersion": 1, "eventSubscriptions": 0});
    if !senha_obs.is_null() {
        let sal = senha_obs["salt"].as_str().ok_or("OBS não informou salt")?;
        let desafio = senha_obs["challenge"].as_str().ok_or("OBS não informou challenge")?;
        if senha.is_empty() {
            return Err("OBS exige senha. Informe-a em Ajustes > Gravações de TV no Saimo (não será salva).".into());
        }
        identificar["authentication"] = json!(assinatura(senha, sal, desafio));
    }
    enviar(&mut ws, json!({"op": 1, "d": identificar}))?;
    let identificado = receber(&mut ws)?;
    if identificado["op"] != 2 {
        return Err("OBS rejeitou a autenticação. Confira a senha em Ferramentas > Servidor WebSocket.".into());
    }

    let mut serial = 0;
    let status = chamar(&mut ws, "GetRecordStatus", &mut serial)?;
    let ativo = status["outputActive"].as_bool().unwrap_or(false);
    if gravar {
        if ativo {
            return Err("O OBS já está gravando. O Saimo não vai interromper uma gravação iniciada por outro programa.".into());
        }
        let selecionada = chamar(&mut ws, "GetCurrentProgramScene", &mut serial)?;
        let atual = selecionada["currentProgramSceneName"].as_str().unwrap_or("");
        if atual != cena {
            return Err(format!(
                "Cena atual do OBS: '{atual}'. Selecione a cena '{cena}' com captura de janela e áudio do Saimo."
            ));
        }
        chamar(&mut ws, "StartRecord", &mut serial)?;
        std::thread::sleep(Duration::from_millis(1400));
        let status = chamar(&mut ws, "GetRecordStatus", &mut serial)?;
        if !status["outputActive"].as_bool().unwrap_or(false) {
            return Err("OBS não confirmou gravação ativa. Confira as configurações de saída.".into());
        }
        let bytes = status["outputBytes"].as_u64().unwrap_or(0);
        let mensagem = if bytes > 0 {
            format!("OBS gravando a cena '{cena}' — {bytes} bytes confirmados. O arquivo será salvo na pasta configurada no OBS.")
        } else {
            format!("OBS iniciou a gravação da cena '{cena}', mas ainda não confirmou bytes. Confira áudio e vídeo no OBS.")
        };
        Ok(Resultado { gravando: true, mensagem })
    } else {
        if !ativo {
            return Err("O OBS não está gravando".into());
        }
        let resposta = chamar(&mut ws, "StopRecord", &mut serial)?;
        let caminho = resposta["outputPath"].as_str().unwrap_or("");
        let arquivo = std::path::Path::new(caminho);
        // OBS finaliza o container, que pode levar mais alguns milissegundos.
        let mut tamanho = 0;
        for _ in 0..8 {
            tamanho = std::fs::metadata(arquivo).map(|m| m.len()).unwrap_or(0);
            if tamanho > 0 { break; }
            std::thread::sleep(Duration::from_millis(250));
        }
        if tamanho == 0 {
            return Err(format!("O OBS encerrou, mas não foi possível validar o arquivo. Caminho reportado: {caminho}"));
        }
        Ok(Resultado {
            gravando: false,
            mensagem: format!("OBS finalizou: {caminho} ({tamanho} bytes)."),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn assinatura_do_protocolo_obs_e_estavel() {
        let x = assinatura("supersecretpassword", "lM1GncleQOaCu9lT1yeUZhFYnqhsLLP1G5lAGo3ixaI=", "+IxH4CnCiqpX1rM9scsNynZzbOe4KhDeYcTNS3PDaeY=");
        assert_eq!(x, "Dj6cLS+jrNA0HpCArRg0Z/Fc+YHdt2FQfAvgD1mip6Y=");
    }
}
