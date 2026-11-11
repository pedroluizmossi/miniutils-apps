# Repositórios (app externo do MiniUtils)

Os seus repositórios Git e o que falta em cada um: alterações não commitadas, commits não
enviados (↑), commits do remoto que você ainda não tem (↓), stash e merge/rebase pela
metade. Cada linha tem botões para abrir um terminal, a pasta ou o editor (VS Code,
VSCodium ou Zed, se houver) no repositório.

**Acesso total:** é um programa próprio, que roda com as permissões do seu usuário. Ele lê
as pastas vigiadas e roda o `git` em cada repositório; só escreve o próprio `pastas.txt`.

## A mascote (polvo)

- **Alarme:** conflito, ou merge/rebase/cherry-pick pela metade.
- **Agitada:** alguma pendência (não commitado ou não enviado) mais velha que o limite da
  opção "Horas até a mascote cobrar pendências" (padrão: 24 h).
- **Quieta:** pendências recentes; você está trabalhando.
- **Contente:** tudo commitado e enviado.

Na conversa entre mascotes, o app conta quando um repositório fica com pendência velha ou
em conflito, e quando tudo volta a ficar em dia.

## Pastas vigiadas

Na primeira vez, o app cria `pastas.txt` na pasta de dados dele, com as pastas de projetos
que achar na sua pasta pessoal (`~/Projetos`, `~/projects`, `~/src`, `~/code`…). O botão
**Pastas** abre esse arquivo: uma pasta por linha, `~` é a pasta pessoal. Salve e clique em
**Atualizar**. A busca desce até a profundidade da opção "Profundidade da busca nas
pastas" (padrão: 2) e não entra em pastas ocultas, `node_modules`, `target` e afins.

## Quando atualiza

- **Na hora:** commit, add, checkout, pull, fetch, ou qualquer coisa que mexa dentro do
  `.git`. O app vigia só o `.git`, não a árvore de trabalho, para não gastar milhares de
  vigias com `node_modules`.
- **A cada 2 minutos, e quando a janela do hub abre:** todos (é assim que um arquivo
  editado aparece).
- **A cada 20 minutos, e no Atualizar:** procura repositórios novos nas pastas.
- **A cada 30 minutos, com a opção "Buscar no remoto" ligada:** `git fetch` em quem tem
  remoto, sem pedir senha (sem chave carregada, o fetch falha em silêncio e vai para o
  registro do hub).

O app usa o próprio `git` (precisa estar no PATH), com `--no-optional-locks`: a leitura
não mexe no repositório.
