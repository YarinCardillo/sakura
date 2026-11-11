# Security

## Reporting

Report a vulnerability privately through
[GitHub's advisory form](https://github.com/YarinCardillo/sakura/security/advisories/new). Please do
not open a public issue for it. Say what you ran, on which system, and what an agent or a repository
could do that it should not. You will get an answer within a week.

## Threat model

1. A box holds a hostile agent. Everything it writes (its clone, its `.git`, its logs, its requests)
   is untrusted, and the limits on it come from Docker, never from its cooperation.
2. A box must not reach your home folder, your keys, the Docker socket, your machine's ports or any
   host that is not on the proxy's allow list. A way to do so is a vulnerability.
3. The host never runs git inside a box's clone; what is read from it is read by a container with no
   network and validated before use. Getting the host to execute something a box wrote is a
   vulnerability.
4. A check in a box runs on a clean checkout of one commit with read-only dependencies. A box that can
   change the recorded result of a check, or the code a check runs, is a vulnerability.
5. Nothing leaves a box except through Take, which fetches one branch. Any other way out is a
   vulnerability.
6. A task on your machine runs as you. The agent there has your permissions, your git and your
   network: nothing confines it, by design. What it can do is not a vulnerability in Sakura.
7. Between the two, separation holds: no container ever mounts a task that lives on your machine.
   Breaking that is a vulnerability.
8. The app runs only the `box` command, with arguments it validates, and never a shell line. Making
   it run anything else through a task's name, a path or a file's content is a vulnerability.
9. The app and the service never read or store your credentials; a box gets only the login of the
   account you chose for it. A box reading another account's login is a vulnerability.
10. Out of scope: the agents' own behaviour, the model providers, Docker itself, and a person with
    your user account on your machine.
